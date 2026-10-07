//! `plan()` and `apply()`: from manifest + answers to a staged, ready-to-write
//! solution.
//!
//! `plan()` does all the work in memory and touches the disk only to read the
//! templates and to look at the target location. It returns data: the staged
//! files, warnings, and descriptors of the steps the host should run
//! afterwards. `apply()` is the only place that writes.

use crate::error::{Result, ScaffoldError};
use crate::manifest::{
    when_holds, Manifest, MergeStrategy, NameRule, PackageManager, PackageManagers, PostAction,
    PromptKind,
};
use crate::secrets::{RandomSecrets, SecretSource};
use crate::stage::{normalize_rel, StagedFs, WriteOutcome, WriteProgress};
use crate::tokens::{render, GuidSource, RandomGuids, TokenMap};
use crate::validate::{validate_solution_name, validate_webpart_names, ValidationIssue};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Answers from the panel, keyed by prompt key. Text and choice are JSON
/// strings, bool is a JSON bool, list is an array of strings.
pub type Answers = BTreeMap<String, Value>;

/// Windows' classic path limit.
pub const WINDOWS_MAX_PATH: usize = 260;
/// Rough room `node_modules` needs below the solution folder.
pub const NODE_MODULES_HEADROOM: usize = 140;

/// Everything `plan()` needs besides the manifest, templates and answers.
pub struct PlanOptions {
    /// Folder the user picked. The solution is created *inside* it.
    pub location: PathBuf,
    /// Package manager for `install` steps. Falls back to the manifest default, then npm.
    pub package_manager: Option<PackageManager>,
    pub guids: Box<dyn GuidSource>,
    pub secrets: Box<dyn SecretSource>,
    /// Produce the Windows long-path warning. Defaults to "only on Windows".
    pub check_long_paths: bool,
}

impl PlanOptions {
    pub fn new(location: impl Into<PathBuf>) -> Self {
        Self {
            location: location.into(),
            package_manager: None,
            guids: Box::new(RandomGuids),
            secrets: Box::new(RandomSecrets),
            check_long_paths: cfg!(windows),
        }
    }
}

/// Something the panel should show before the user confirms. Plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The location is inside an existing solution (the manifest's `nestedMarker` was found).
    NestedSolution { found_in: PathBuf },
    /// `install` would build paths longer than Windows' classic limit.
    LongPath { length: usize, limit: usize },
    /// The chosen package manager is not one the generator was tested with.
    UntestedPackageManager(PackageManager),
}

/// A step for the host to run (through its own script runner). The crate never runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostStepDescriptor {
    pub action: PostAction,
    /// Absolute folder to run in.
    pub cwd: PathBuf,
    /// Set for `install` only.
    pub package_manager: Option<PackageManager>,
}

/// The result of `plan()`.
pub struct Plan {
    /// Absolute path of the folder that `apply()` will create.
    pub target: PathBuf,
    /// Name of that folder (the answer to `folderFrom`).
    pub folder_name: String,
    /// Everything that will be written, rendered and merged.
    pub files: StagedFs,
    pub warnings: Vec<Warning>,
    pub post: Vec<PostStepDescriptor>,
}

// Hand-written so that logging a plan can never print file contents
// (which may hold generated secrets).
impl fmt::Debug for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plan")
            .field("target", &self.target)
            .field("files", &self.files.len())
            .field("warnings", &self.warnings)
            .field("post", &self.post)
            .finish()
    }
}

/// Builds the plan. Nothing is written.
pub fn plan(
    manifest: &Manifest,
    template_dir: &Path,
    answers: &Answers,
    mut options: PlanOptions,
) -> Result<Plan> {
    let answers = resolve_answers(manifest, answers)?;

    // ---- target folder
    let folder_key = manifest
        .folder_from
        .as_deref()
        .ok_or_else(|| ScaffoldError::Manifest("the manifest has no 'folderFrom'".into()))?;
    let folder_name = answers
        .get(folder_key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let clean = normalize_rel(&folder_name)?;
    if clean.contains('/') {
        return Err(ScaffoldError::InvalidPath {
            path: folder_name,
            reason: "the folder name must be a single name".into(),
        });
    }
    if !options.location.is_dir() {
        return Err(ScaffoldError::InvalidPath {
            path: options.location.display().to_string(),
            reason: "the location does not exist".into(),
        });
    }
    let target = options.location.join(&folder_name);
    if target.exists() {
        return Err(ScaffoldError::FolderExists(target));
    }

    // ---- tokens
    let mut base = TokenMap::new();
    for p in &manifest.prompts {
        let value = &answers[&p.key];
        match p.kind {
            PromptKind::Text => {
                let text = value.as_str().unwrap_or_default();
                if p.sensitive {
                    base.insert(&p.key, text)?;
                } else {
                    base.insert_name(&p.key, text)?;
                }
            }
            PromptKind::Choice => base.insert(&p.key, value.as_str().unwrap_or_default())?,
            PromptKind::Bool | PromptKind::List => {}
        }
    }
    for g in &manifest.guids {
        base.insert_guid(g, options.guids.as_mut())?;
    }
    for s in &manifest.secrets {
        base.insert(s, options.secrets.next_secret())?;
    }

    let mut files = StagedFs::new();

    // ---- include: folders copied once
    for inc in manifest.include.iter().filter(|i| when_holds(&i.when, &answers)) {
        copy_folder(
            &mut files,
            template_dir,
            &inc.folder,
            inc.to.as_deref(),
            &base,
        )?;
    }

    // ---- repeat: folders per list item. Remember each item's tokens for merges.
    let mut item_tokens: Vec<(String, Vec<TokenMap>)> = Vec::new(); // (over, per-item maps)
    for r in manifest.repeat.iter().filter(|r| when_holds(&r.when, &answers)) {
        let items = list_items(&answers, &r.over);
        let mut maps = Vec::with_capacity(items.len());
        for item in &items {
            let mut tokens = base.clone();
            tokens.insert_name(&r.item, item)?;
            for g in &r.guids {
                tokens.insert_guid(g, options.guids.as_mut())?;
            }
            copy_folder(&mut files, template_dir, &r.folder, None, &tokens)?;
            maps.push(tokens);
        }
        item_tokens.push((r.over.clone(), maps));
    }

    // ---- parts: assembled text files
    let mut assembled: Vec<(String, String)> = Vec::new();
    for part in manifest.parts.iter().filter(|p| when_holds(&p.when, &answers)) {
        let target_path = render(&part.target, &base, "part target")?;
        let text = read_template_text(template_dir, &part.source)?;
        let mut text = render(&text, &base, &part.source)?;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        match assembled.iter_mut().find(|(t, _)| *t == target_path) {
            Some((_, content)) => content.push_str(&text),
            None => assembled.push((target_path, text)),
        }
    }
    for (path, content) in assembled {
        files.insert_text(&path, &content)?;
    }

    // ---- merge
    for m in manifest.merge.iter().filter(|m| when_holds(&m.when, &answers)) {
        let maps: Vec<&TokenMap> = match &m.per {
            None => vec![&base],
            Some(per) => match item_tokens.iter().find(|(over, _)| over == per) {
                Some((_, maps)) => maps.iter().collect(),
                None => continue, // no active repeat over this list
            },
        };
        for tokens in maps {
            let file = render(&m.file, tokens, "merge file")?;
            let path = render(&m.path, tokens, &format!("merge path in {file}"))?;
            let value = match (&m.value, &m.value_file) {
                (Some(v), _) => render_json(v, tokens, &format!("merge value for {file}"))?,
                (None, Some(vf)) => {
                    let text = read_template_text(template_dir, vf)?;
                    let text = render(&text, tokens, vf)?;
                    serde_json::from_str(&text).map_err(|e| ScaffoldError::Merge {
                        file: file.clone(),
                        reason: format!("'{vf}' is not valid JSON after rendering: {e}"),
                    })?
                }
                (None, None) => unreachable_manifest(&file)?,
            };
            apply_merge(&mut files, &file, m.strategy, &path, value)?;
        }
    }

    // ---- post steps and warnings
    let fallback = PackageManagers::fallback();
    let pm_info = manifest.package_managers.as_ref().unwrap_or(&fallback);
    let chosen_pm = options.package_manager.unwrap_or(pm_info.default);

    let mut post = Vec::new();
    for step in manifest.post.iter().filter(|s| when_holds(&s.when, &answers)) {
        let cwd = match &step.cwd {
            Some(c) => target.join(normalize_rel(c)?),
            None => target.clone(),
        };
        post.push(PostStepDescriptor {
            action: step.action,
            cwd,
            package_manager: (step.action == PostAction::Install).then_some(chosen_pm),
        });
    }

    let mut warnings = Vec::new();
    if let Some(marker) = &manifest.nested_marker {
        if let Some(found) = options
            .location
            .ancestors()
            .find(|dir| dir.join(marker).is_file())
        {
            warnings.push(Warning::NestedSolution {
                found_in: found.to_path_buf(),
            });
        }
    }
    let installs = post.iter().any(|p| p.action == PostAction::Install);
    if installs {
        if !pm_info.is_tested(chosen_pm) {
            warnings.push(Warning::UntestedPackageManager(chosen_pm));
        }
        if options.check_long_paths {
            let length = target.as_os_str().len() + NODE_MODULES_HEADROOM;
            if length >= WINDOWS_MAX_PATH {
                warnings.push(Warning::LongPath {
                    length,
                    limit: WINDOWS_MAX_PATH,
                });
            }
        }
    }

    Ok(Plan {
        target,
        folder_name,
        files,
        warnings,
        post,
    })
}

/// Writes the plan into its new folder. Fails if the folder exists, and removes
/// the folder again if writing fails halfway.
pub fn apply(plan: &Plan, on_progress: impl FnMut(WriteProgress)) -> Result<WriteOutcome> {
    plan.files.write_to(&plan.target, on_progress)
}

// ---------------------------------------------------------------- answers

/// Applies defaults and checks every answer against its prompt.
fn resolve_answers(manifest: &Manifest, given: &Answers) -> Result<Answers> {
    let known: HashSet<&str> = manifest.prompts.iter().map(|p| p.key.as_str()).collect();
    let mut unknown: Vec<&str> = given
        .keys()
        .map(String::as_str)
        .filter(|k| !known.contains(k))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(ScaffoldError::Answers(format!(
            "unknown answer(s): {}",
            unknown.join(", ")
        )));
    }

    let mut out = Answers::new();
    let mut problems: Vec<String> = Vec::new();
    let mut issues: Vec<ValidationIssue> = Vec::new();

    for p in &manifest.prompts {
        let raw = given.get(&p.key).or(p.default.as_ref());
        let value = match p.kind {
            PromptKind::Text => match raw {
                Some(Value::String(s)) if !s.trim().is_empty() => Value::String(s.trim().to_string()),
                Some(Value::String(_)) | None => {
                    problems.push(format!("'{}' is required", p.key));
                    continue;
                }
                Some(_) => {
                    problems.push(format!("'{}' must be text", p.key));
                    continue;
                }
            },
            PromptKind::Bool => match raw {
                Some(Value::Bool(b)) => Value::Bool(*b),
                None => Value::Bool(false),
                Some(_) => {
                    problems.push(format!("'{}' must be true or false", p.key));
                    continue;
                }
            },
            PromptKind::Choice => match raw {
                Some(Value::String(s)) if p.options.iter().any(|o| o == s) => Value::String(s.clone()),
                Some(_) => {
                    problems.push(format!(
                        "'{}' must be one of: {}",
                        p.key,
                        p.options.join(", ")
                    ));
                    continue;
                }
                None => {
                    problems.push(format!("'{}' is required", p.key));
                    continue;
                }
            },
            PromptKind::List => match raw {
                None => Value::Array(Vec::new()),
                Some(Value::Array(items)) => {
                    let mut list = Vec::with_capacity(items.len());
                    let mut ok = true;
                    for item in items {
                        match item.as_str().map(str::trim) {
                            Some(s) if !s.is_empty() => list.push(Value::String(s.to_string())),
                            _ => {
                                problems.push(format!("'{}' has an empty or non-text item", p.key));
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    Value::Array(list)
                }
                Some(_) => {
                    problems.push(format!("'{}' must be a list of text", p.key));
                    continue;
                }
            },
        };

        match (p.rule, &value) {
            (Some(NameRule::SolutionName), Value::String(s)) => {
                issues.extend(retarget(validate_solution_name(s), &p.key));
            }
            (Some(NameRule::WebpartNames), Value::Array(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                issues.extend(retarget(validate_webpart_names(&names), &p.key));
            }
            _ => {}
        }
        out.insert(p.key.clone(), value);
    }

    if !problems.is_empty() {
        return Err(ScaffoldError::Answers(problems.join("; ")));
    }
    if !issues.is_empty() {
        return Err(ScaffoldError::Validation(issues));
    }
    Ok(out)
}

/// The validators name their field generically; point it at the prompt key.
fn retarget(issues: Vec<ValidationIssue>, key: &str) -> Vec<ValidationIssue> {
    issues
        .into_iter()
        .map(|i| {
            let field = match i.field.find('[') {
                Some(pos) => format!("{key}{}", &i.field[pos..]),
                None => key.to_string(),
            };
            ValidationIssue {
                field,
                message: i.message,
            }
        })
        .collect()
}

fn list_items(answers: &Answers, key: &str) -> Vec<String> {
    answers
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

// --------------------------------------------------------------- templates

/// Copies a template folder into the stage, rendering paths and text contents.
fn copy_folder(
    files: &mut StagedFs,
    template_dir: &Path,
    folder: &str,
    to: Option<&str>,
    tokens: &TokenMap,
) -> Result<()> {
    let folder = normalize_rel(folder)?;
    let root = template_dir.join(&folder);
    let mut found = Vec::new();
    walk(&root, "", &mut found, &folder)?;
    for rel in found {
        let source = format!("{folder}/{rel}");
        let mut dest = render(&rel, tokens, &format!("path {source}"))?;
        if let Some(to) = to {
            dest = format!("{}/{dest}", to.trim_matches('/'));
        }
        let bytes = fs::read(template_dir.join(&folder).join(&rel)).map_err(|e| ScaffoldError::Template {
            path: source.clone(),
            reason: e.to_string(),
        })?;
        // Text is rendered; anything that is not valid UTF-8 is copied as is.
        let bytes = match String::from_utf8(bytes) {
            Ok(text) => render(&text, tokens, &source)?.into_bytes(),
            Err(e) => e.into_bytes(),
        };
        files.insert(&dest, bytes)?;
    }
    Ok(())
}

/// Lists all files below `dir` (relative, `/`-separated, sorted). Symlinks are refused.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>, label: &str) -> Result<()> {
    let read = fs::read_dir(dir).map_err(|e| ScaffoldError::Template {
        path: label.to_string(),
        reason: format!("cannot read template folder: {e}"),
    })?;
    let mut entries: Vec<_> = read
        .collect::<std::io::Result<_>>()
        .map_err(|e| ScaffoldError::Template {
            path: label.to_string(),
            reason: e.to_string(),
        })?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| ScaffoldError::Template {
            path: format!("{label}/{rel}"),
            reason: e.to_string(),
        })?;
        if meta.file_type().is_symlink() {
            return Err(ScaffoldError::Template {
                path: format!("{label}/{rel}"),
                reason: "symbolic links are not allowed in templates".into(),
            });
        }
        if meta.is_dir() {
            walk(&entry.path(), &rel, out, label)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

fn read_template_text(template_dir: &Path, rel: &str) -> Result<String> {
    let rel = normalize_rel(rel)?;
    let path = template_dir.join(&rel);
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            return Err(ScaffoldError::Template {
                path: rel,
                reason: "symbolic links are not allowed in templates".into(),
            });
        }
    }
    fs::read_to_string(&path).map_err(|e| ScaffoldError::Template {
        path: rel,
        reason: e.to_string(),
    })
}

// ------------------------------------------------------------------- merge

/// Renders every string inside a JSON value. Types are preserved: a string
/// stays a string (use `valueFile` when a token must become a number).
fn render_json(value: &Value, tokens: &TokenMap, context: &str) -> Result<Value> {
    Ok(match value {
        Value::String(s) => Value::String(render(s, tokens, context)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| render_json(v, tokens, context))
                .collect::<Result<_>>()?,
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(render(k, tokens, context)?, render_json(v, tokens, context)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

fn unreachable_manifest(file: &str) -> Result<Value> {
    Err(ScaffoldError::Merge {
        file: file.to_string(),
        reason: "the manifest gives neither 'value' nor 'valueFile'".into(),
    })
}

fn apply_merge(
    files: &mut StagedFs,
    file: &str,
    strategy: MergeStrategy,
    path: &str,
    value: Value,
) -> Result<()> {
    let fail = |reason: String| ScaffoldError::Merge {
        file: file.to_string(),
        reason,
    };
    let text = files
        .get_text(file)
        .ok_or_else(|| fail("the file is not part of the output (or is not text)".into()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut doc: Value =
        serde_json::from_str(text).map_err(|e| fail(format!("not valid JSON: {e}")))?;

    let segments: Vec<&str> = path.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(fail(format!("'{path}' is not a valid dotted path")));
    }
    let (last, parents) = segments.split_last().expect("split always yields one item");

    let mut node = &mut doc;
    for seg in parents {
        let obj = node
            .as_object_mut()
            .ok_or_else(|| fail(format!("'{seg}' in '{path}' is not inside an object")))?;
        node = obj
            .entry((*seg).to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    let obj = node
        .as_object_mut()
        .ok_or_else(|| fail(format!("the parent of '{last}' in '{path}' is not an object")))?;

    match strategy {
        MergeStrategy::JsonSet => {
            obj.insert((*last).to_string(), value);
        }
        MergeStrategy::JsonAppend => {
            let slot = obj
                .entry((*last).to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            slot.as_array_mut()
                .ok_or_else(|| fail(format!("'{path}' exists but is not an array")))?
                .push(value);
        }
    }

    let mut out = serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?;
    out.push('\n');
    files.replace(file, out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::secrets::SequentialSecrets;
    use crate::tokens::SequentialGuids;
    use serde_json::json;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-plan-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn opts(location: &Path) -> PlanOptions {
        let mut o = PlanOptions::new(location);
        o.guids = Box::new(SequentialGuids::new());
        o.secrets = Box::new(SequentialSecrets::new());
        o.check_long_paths = true;
        o
    }

    fn answers(v: Value) -> Answers {
        v.as_object().unwrap().clone().into_iter().collect()
    }

    // ---------------------------------------------------------------- SPFx

    const SPFX: &str = r#"{
      "id": "spfx-webpart",
      "spfxVersion": "1.23.2",
      "folderFrom": "solutionName",
      "nestedMarker": ".yo-rc.json",
      "prompts": [
        { "key": "solutionName", "type": "text", "rule": "solution-name" },
        { "key": "webparts", "type": "list", "rule": "webpart-names" },
        { "key": "install", "type": "bool", "default": false }
      ],
      "guids": ["solutionId"],
      "include": [ { "folder": "solution" } ],
      "repeat": [
        { "over": "webparts", "as": "webpartNaam", "folder": "webpart", "guids": ["webpartId"] }
      ],
      "merge": [
        { "file": "config/config.json", "strategy": "json-set", "per": "webparts",
          "path": "bundles.{__webpartNaamKebab__}-web-part",
          "value": { "components": [ { "entrypoint": "./lib/webparts/{__webpartNaamCamel__}/{__webpartNaam__}WebPart.js" } ] } },
        { "file": "package-solution.json", "strategy": "json-set", "path": "solution.id", "value": "{__solutionId__}" }
      ],
      "post": [ { "action": "install", "when": { "key": "install", "equals": true } } ],
      "packageManagers": { "default": "npm", "tested": ["npm"] }
    }"#;

    fn spfx_templates() -> TempDir {
        let t = TempDir::new();
        t.write("solution/.yo-rc.json", "{\n  \"@microsoft/generator-sharepoint\": { \"solutionName\": \"{__solutionName__}\" }\n}\n");
        t.write("solution/package.json", "{ \"name\": \"{__solutionName__}\" }\n");
        t.write("solution/package-solution.json", "{\n  \"solution\": { \"name\": \"{__solutionName__}-client-side-solution\" }\n}\n");
        t.write("solution/config/config.json", "{\n  \"bundles\": {}\n}\n");
        t.write("webpart/src/webparts/{__webpartNaamCamel__}/{__webpartNaam__}WebPart.manifest.json", "{ \"id\": \"{__webpartId__}\", \"alias\": \"{__webpartNaam__}WebPart\" }\n");
        t.write("webpart/src/webparts/{__webpartNaamCamel__}/components/{__webpartNaam__}.tsx", "export const {__webpartNaam__} = () => null;\n");
        t
    }

    #[test]
    fn spfx_like_plan_renders_repeats_merges_and_guids() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld", "News"] }));
        let plan = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();

        assert_eq!(plan.folder_name, "my-solution");
        assert_eq!(plan.target, loc.0.join("my-solution"));

        let paths: Vec<&str> = plan.files.paths().collect();
        assert!(paths.contains(&"package.json"));
        assert!(paths.contains(&"src/webparts/helloWorld/HelloWorldWebPart.manifest.json"));
        assert!(paths.contains(&"src/webparts/news/components/News.tsx"));
        assert_eq!(paths.len(), 4 + 4);

        assert_eq!(
            plan.files.get_text("src/webparts/news/components/News.tsx").unwrap(),
            "export const News = () => null;\n"
        );

        // GUIDs: solution first, then one per item, all different.
        let hello = plan.files.get_text("src/webparts/helloWorld/HelloWorldWebPart.manifest.json").unwrap();
        let news = plan.files.get_text("src/webparts/news/NewsWebPart.manifest.json").unwrap();
        assert!(hello.contains("00000000-0000-0000-0000-000000000002"), "{hello}");
        assert!(news.contains("00000000-0000-0000-0000-000000000003"), "{news}");

        // Per-item merge into config.json, with key order preserved.
        let config: Value = serde_json::from_str(plan.files.get_text("config/config.json").unwrap()).unwrap();
        let bundles = config["bundles"].as_object().unwrap();
        assert_eq!(bundles.keys().collect::<Vec<_>>(), ["hello-world-web-part", "news-web-part"]);
        assert_eq!(
            bundles["news-web-part"]["components"][0]["entrypoint"],
            "./lib/webparts/news/NewsWebPart.js"
        );

        // Once-only merge with a solution-level GUID, keeping the existing keys.
        let sol: Value = serde_json::from_str(plan.files.get_text("package-solution.json").unwrap()).unwrap();
        assert_eq!(sol["solution"]["id"], "00000000-0000-0000-0000-000000000001");
        assert_eq!(sol["solution"]["name"], "my-solution-client-side-solution");

        assert!(plan.post.is_empty());
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn planning_is_deterministic_with_sequential_sources() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"] }));
        let one = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        let two = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        let paths: Vec<_> = one.files.paths().map(str::to_string).collect();
        for p in &paths {
            assert_eq!(one.files.get(p), two.files.get(p), "{p}");
        }
    }

    #[test]
    fn install_step_is_a_descriptor_with_warnings() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"], "install": true }));

        let mut o = opts(&loc.0);
        o.package_manager = Some(PackageManager::Pnpm);
        let p = plan(&m, &t.0, &a, o).unwrap();
        assert_eq!(p.post.len(), 1);
        assert_eq!(p.post[0].action, PostAction::Install);
        assert_eq!(p.post[0].package_manager, Some(PackageManager::Pnpm));
        assert_eq!(p.post[0].cwd, p.target);
        assert!(p.warnings.contains(&Warning::UntestedPackageManager(PackageManager::Pnpm)));
        // nothing was written
        assert!(!p.target.exists());

        // default package manager is the tested one: no warning
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        assert_eq!(p.post[0].package_manager, Some(PackageManager::Npm));
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn nested_solution_and_long_path_warnings() {
        let t = spfx_templates();
        let outer = TempDir::new();
        outer.write(".yo-rc.json", "{}");
        outer.write("inner/.keep", "");
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"], "install": true }));

        let p = plan(&m, &t.0, &a, opts(&outer.0.join("inner"))).unwrap();
        assert!(p.warnings.contains(&Warning::NestedSolution { found_in: outer.0.clone() }));

        // A deep location triggers the long-path warning, only when install is on.
        let deep = outer.0.join("a".repeat(100)).join("b".repeat(30));
        fs::create_dir_all(&deep).unwrap();
        let p = plan(&m, &t.0, &a, opts(&deep)).unwrap();
        assert!(p.warnings.iter().any(|w| matches!(w, Warning::LongPath { .. })));
        let no_install = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"] }));
        let p = plan(&m, &t.0, &no_install, opts(&deep)).unwrap();
        assert!(!p.warnings.iter().any(|w| matches!(w, Warning::LongPath { .. })));
    }

    #[test]
    fn bad_answers_and_bad_names_are_reported_before_anything_happens() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();

        let err = plan(&m, &t.0, &answers(json!({ "webparts": ["A"] })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("'solutionName' is required"), "{err}");

        let err = plan(&m, &t.0, &answers(json!({ "solutionName": "x", "webparts": ["A"], "nope": 1 })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("unknown answer"), "{err}");

        let err = plan(&m, &t.0, &answers(json!({ "solutionName": "My Solution", "webparts": ["1bad", "1bad"] })), opts(&loc.0)).unwrap_err();
        match err {
            ScaffoldError::Validation(issues) => {
                assert!(issues.iter().any(|i| i.field == "solutionName"));
                assert!(issues.iter().any(|i| i.field.starts_with("webparts[")));
            }
            other => panic!("expected Validation, got {other}"),
        }

        let err = plan(&m, &t.0, &answers(json!({ "solutionName": "ok", "webparts": [] })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("at least one web part"), "{err}");
    }

    #[test]
    fn existing_folder_and_missing_location_are_errors() {
        let t = spfx_templates();
        let loc = TempDir::new();
        fs::create_dir(loc.0.join("my-solution")).unwrap();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["A"] }));
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err();
        assert!(matches!(err, ScaffoldError::FolderExists(_)));
        assert!(err.to_string().contains("please choose a different name"));

        let err = plan(&m, &t.0, &a, opts(&loc.0.join("missing"))).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn unknown_tokens_in_templates_are_an_error_with_the_file_named() {
        let t = spfx_templates();
        t.write("solution/oops.txt", "{__doesNotExist__}");
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "x", "webparts": ["A"] }));
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("doesNotExist") && err.contains("solution/oops.txt"), "{err}");
    }

    #[test]
    fn merging_into_a_missing_or_broken_file_is_an_error() {
        let t = spfx_templates();
        t.write("solution/config/config.json", "{ not json");
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "x", "webparts": ["A"] }));
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("config/config.json") && err.contains("not valid JSON"), "{err}");

        fs::remove_file(t.0.join("solution/config/config.json")).unwrap();
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("not part of the output"), "{err}");
    }

    #[test]
    fn apply_writes_the_plan_and_refuses_an_existing_folder() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"] }));
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();

        let mut seen = 0;
        let out = apply(&p, |_| seen += 1).unwrap();
        assert_eq!(out.files_written, p.files.len());
        assert_eq!(seen, p.files.len());
        assert!(loc.0.join("my-solution/src/webparts/helloWorld/components/HelloWorld.tsx").is_file());

        let err = apply(&p, |_| {}).unwrap_err();
        assert!(matches!(err, ScaffoldError::FolderExists(_)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_templates_are_refused() {
        let t = spfx_templates();
        std::os::unix::fs::symlink("/etc/passwd", t.0.join("solution/leak")).unwrap();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "x", "webparts": ["A"] }));
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("symbolic links"), "{err}");
    }

    // ----------------------------------------------------------- full stack

    const FULLSTACK: &str = r#"{
      "id": "fullstack",
      "spfxVersion": "1.23.2",
      "folderFrom": "projectName",
      "prompts": [
        { "key": "projectName", "type": "text", "rule": "solution-name" },
        { "key": "preset", "type": "choice", "options": ["docker", "hybrid", "native"], "default": "docker" },
        { "key": "db", "type": "choice", "options": ["postgres", "sqlite", "mssql"], "default": "postgres" },
        { "key": "dbPort", "type": "text", "default": "5432" },
        { "key": "dbPassword", "type": "text", "sensitive": true, "default": "ignored-in-docker" },
        { "key": "flow", "type": "bool", "default": true }
      ],
      "secrets": ["jwtSecret"],
      "include": [
        { "folder": "base" },
        { "folder": "flow", "when": { "key": "flow", "equals": true } }
      ],
      "parts": [
        { "target": "docker-compose.yml", "source": "compose/head.yml", "when": { "key": "preset", "in": ["docker", "hybrid"] } },
        { "target": "docker-compose.yml", "source": "compose/postgres.yml", "when": { "all": [ { "key": "preset", "in": ["docker", "hybrid"] }, { "key": "db", "equals": "postgres" } ] } },
        { "target": "docker-compose.yml", "source": "compose/mssql.yml", "when": { "all": [ { "key": "preset", "in": ["docker", "hybrid"] }, { "key": "db", "equals": "mssql" } ] } },
        { "target": "docker-compose.yml", "source": "compose/app.yml", "when": { "key": "preset", "equals": "docker" } }
      ],
      "merge": [
        { "file": ".zed/workflows/flow-{__projectNameKebab__}.flow.json", "strategy": "json-set",
          "path": "actions.db", "valueFile": "fragments/db-action.json",
          "when": { "all": [ { "key": "flow", "equals": true }, { "key": "preset", "in": ["hybrid", "docker"] } ] } }
      ],
      "post": [
        { "action": "composeUp", "when": { "key": "preset", "in": ["docker", "hybrid"] } },
        { "action": "install", "cwd": "client", "when": { "key": "preset", "in": ["hybrid", "native"] } }
      ]
    }"#;

    fn fullstack_templates() -> TempDir {
        let t = TempDir::new();
        t.write("base/.env", "DB_PORT={__dbPort__}\nJWT_SECRET={__jwtSecret__}\nPROJECT={__projectName__}\n");
        t.write("base/client/package.json", "{ \"name\": \"{__projectNameKebab__}-client\" }\n");
        t.write("base/logo.bin", "");
        fs::write(t.0.join("base/logo.bin"), [0xff, 0xfe, 0x00, 0x7b, 0x5f]).unwrap();
        t.write("flow/.zed/workflows/flow-{__projectNameKebab__}.flow.json", "{ \"id\": \"flow-{__projectNameKebab__}\", \"actions\": {} }\n");
        t.write("compose/head.yml", "services:");
        t.write("compose/postgres.yml", "  db:\n    image: postgres:16\n    ports: [\"{__dbPort__}:5432\"]");
        t.write("compose/mssql.yml", "  db:\n    image: mssql\n");
        t.write("compose/app.yml", "  server:\n    env_file: .env\n");
        t.write("fragments/db-action.json", "{ \"type\": \"StartProcess\", \"inputs\": { \"port\": {__dbPort__} }, \"pos\": [0, 80] }");
        t
    }

    #[test]
    fn fullstack_like_plan_with_parts_value_files_and_secrets() {
        let t = fullstack_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(FULLSTACK).unwrap();
        let a = answers(json!({ "projectName": "task-chain", "dbPort": "5433", "preset": "docker" }));
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();

        // secrets are alphanumeric tokens, and the .env is rendered
        let env = p.files.get_text(".env").unwrap();
        assert!(env.contains("DB_PORT=5433"));
        assert!(env.contains("JWT_SECRET=Aa11x"), "{env}");

        // parts: ordered, filtered by all/in/equals, joined with newlines
        let compose = p.files.get_text("docker-compose.yml").unwrap();
        assert_eq!(
            compose,
            "services:\n  db:\n    image: postgres:16\n    ports: [\"5433:5432\"]\n  server:\n    env_file: .env\n"
        );

        // valueFile keeps the port a number, and the merge path/file carry tokens
        let flow: Value = serde_json::from_str(
            p.files.get_text(".zed/workflows/flow-task-chain.flow.json").unwrap(),
        )
        .unwrap();
        assert_eq!(flow["actions"]["db"]["inputs"]["port"], json!(5433));
        assert_eq!(flow["actions"]["db"]["pos"], json!([0, 80]));

        // non-text files are copied untouched even if they look like tokens
        assert_eq!(p.files.get("logo.bin").unwrap(), &[0xff, 0xfe, 0x00, 0x7b, 0x5f]);

        // docker preset: composeUp only
        assert_eq!(p.post.len(), 1);
        assert_eq!(p.post[0].action, PostAction::ComposeUp);
        assert_eq!(p.post[0].package_manager, None);
    }

    #[test]
    fn presets_and_database_choice_change_the_output() {
        let t = fullstack_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(FULLSTACK).unwrap();

        // hybrid + mssql: no app service, mssql fragment, both post steps
        let a = answers(json!({ "projectName": "task-chain", "preset": "hybrid", "db": "mssql", "flow": false }));
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        assert_eq!(p.files.get_text("docker-compose.yml").unwrap(), "services:\n  db:\n    image: mssql\n");
        assert!(!p.files.contains(".zed/workflows/flow-task-chain.flow.json"));
        let actions: Vec<_> = p.post.iter().map(|s| s.action).collect();
        assert_eq!(actions, vec![PostAction::ComposeUp, PostAction::Install]);
        assert_eq!(p.post[1].cwd, p.target.join("client"));

        // native: no compose file at all
        let a = answers(json!({ "projectName": "task-chain", "preset": "native" }));
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        assert!(!p.files.contains("docker-compose.yml"));

        // a choice answer outside its options is rejected
        let a = answers(json!({ "projectName": "task-chain", "db": "oracle" }));
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("'db' must be one of"), "{err}");
    }

    #[test]
    fn sensitive_values_are_plain_tokens_and_never_printed() {
        let t = fullstack_templates();
        t.write("base/secret.txt", "{__dbPassword__}");
        let loc = TempDir::new();
        let m = Manifest::from_json(FULLSTACK).unwrap();
        let a = answers(json!({ "projectName": "task-chain", "dbPassword": "hunter2-Hunter2" }));
        let p = plan(&m, &t.0, &a, opts(&loc.0)).unwrap();
        assert_eq!(p.files.get_text("secret.txt").unwrap(), "hunter2-Hunter2");

        // Debug output of the plan has no file contents
        let debug = format!("{p:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(!debug.contains("Aa11x"), "{debug}");

        // no derived casings for sensitive tokens
        t.write("base/secret.txt", "{__dbPasswordPascal__}");
        let err = plan(&m, &t.0, &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("dbPasswordPascal"), "{err}");
        assert!(!err.contains("hunter2"), "{err}");
    }

    // ------------------------------------------------------------- manifest

    #[test]
    fn manifest_checks_for_the_new_fields() {
        let bad = |from: &str, to: &str| {
            Manifest::from_json(&FULLSTACK.replacen(from, to, 1)).unwrap_err().to_string()
        };
        assert!(bad("\"folderFrom\": \"projectName\"", "\"folderFrom\": \"flow\"").contains("folderFrom"));
        assert!(bad("{ \"key\": \"flow\", \"type\": \"bool\", \"default\": true }", "{ \"key\": \"flow\", \"type\": \"bool\", \"default\": true, \"sensitive\": true }").contains("only allowed on text"));
        assert!(bad("\"in\": [\"docker\", \"hybrid\"] } },\n        { \"target\": \"docker-compose.yml\", \"source\": \"compose/postgres.yml\"", "\"in\": [\"docker\", \"nope\"] } },\n        { \"target\": \"docker-compose.yml\", \"source\": \"compose/postgres.yml\"").contains("one of its options"));
        assert!(bad("\"valueFile\": \"fragments/db-action.json\"", "\"valueFile\": \"fragments/db-action.json\", \"value\": 1").contains("exactly one of"));
        assert!(bad("\"secrets\": [\"jwtSecret\"]", "\"secrets\": [\"projectName\"]").contains("more than once"));
        assert!(bad("{ \"key\": \"flow\", \"equals\": true }", "{ \"all\": [] }").contains("at least one condition"));
        assert!(bad("{ \"key\": \"flow\", \"equals\": true }", "{ \"key\": \"flow\" }").contains("exactly one of 'equals' and 'in'"));
        assert!(bad("\"cwd\": \"client\"", "\"cwd\": \"../client\"").contains("post cwd"));
    }

    #[test]
    fn when_evaluation() {
        use crate::manifest::When;
        let a = answers(json!({ "db": "mssql", "flow": true }));
        let eq = When { key: Some("db".into()), equals: Some(json!("mssql")), ..Default::default() };
        let inn = When { key: Some("db".into()), any_of: Some(vec![json!("postgres"), json!("mysql")]), ..Default::default() };
        let all = When { all: Some(vec![eq.clone(), When { key: Some("flow".into()), equals: Some(json!(true)), ..Default::default() }]), ..Default::default() };
        assert!(eq.matches(&a));
        assert!(!inn.matches(&a));
        assert!(all.matches(&a));
        assert!(!When { key: Some("missing".into()), equals: Some(json!(1)), ..Default::default() }.matches(&a));
    }
}
