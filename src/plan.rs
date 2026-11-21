//! `plan()` and `apply()`: from manifest + answers to a staged, ready-to-write
//! solution.
//!
//! `plan()` does all the work in memory. It reads templates through a
//! [`TemplateSource`] (a folder or a pack) and looks at the target location on
//! disk, nothing else. It returns data: the staged
//! files, warnings, and descriptors of the steps the host should run
//! afterwards. `apply()` is the only place that writes.

use crate::error::{Result, ScaffoldError};
use crate::manifest::{
    when_holds, Manifest, MergeStrategy, NameRule, PackageManager, PackageManagers, PostAction,
    PromptKind,
};
use crate::secrets::{RandomSecrets, SecretSource};
use crate::source::TemplateSource;
use crate::stage::{normalize_rel, Limits, StagedFs, WriteOutcome, WriteProgress};
use crate::tokens::{render, GuidSource, RandomGuids, TokenMap};
use crate::validate::{validate_package_name, validate_solution_name, validate_webpart_names, ValidationIssue};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::PathBuf;

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
    /// How much the template may make. Defaults to the engine's own limits.
    pub limits: Limits,
}

impl PlanOptions {
    pub fn new(location: impl Into<PathBuf>) -> Self {
        Self {
            location: location.into(),
            package_manager: None,
            guids: Box::new(RandomGuids),
            secrets: Box::new(RandomSecrets),
            check_long_paths: cfg!(windows),
            limits: Limits::default(),
        }
    }
}

/// Something the panel should show before the user confirms. Plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Warning {
    /// The location is inside an existing solution (the manifest's `nestedMarker` was found).
    NestedSolution { found_in: PathBuf },
    /// `install` would build paths longer than Windows' classic limit.
    LongPath { length: usize, limit: usize },
    /// The chosen package manager is not one the generator was tested with.
    UntestedPackageManager(PackageManager),
    /// A `package.json` of the new project has scripts that the package manager runs by itself when it installs
    /// (`preinstall`, `install`, `postinstall`, `prepare`). They are code from the template that runs on the
    /// host's machine at the install step, so the host should show them before it runs one.
    InstallScripts { file: String, scripts: Vec<String> },
    /// A `package.json` of the new project depends on something that is not a version from the package registry
    /// (a URL, a git repository, a local path), so installing fetches or links code from somewhere else.
    UrlDependencies { file: String, packages: Vec<String> },
    /// The new project has files that tools run or obey by themselves, without anyone choosing to run them: git hooks
    /// (`.husky/`), editor tasks that start when a folder opens (`.vscode/tasks.json`), a development container,
    /// `.envrc`, and package manager settings or plugins (`.npmrc`, `.yarnrc*`, `.pnpmfile.*`, `.yarn/plugins/`,
    /// `.yarn/releases/`) that can point an install at another registry or run code. Read them before opening the project
    /// in an editor or installing.
    AutoRunFiles { files: Vec<String> },
}

impl Warning {
    /// The warning as JSON: a `kind`, its details, and the `text` a host can show as it is.
    pub fn to_json(&self) -> Value {
        let mut v = match self {
            Warning::NestedSolution { found_in } => serde_json::json!({ "kind": "nested_solution", "foundIn": found_in.display().to_string() }),
            Warning::LongPath { length, limit } => serde_json::json!({ "kind": "long_path", "length": length, "limit": limit }),
            Warning::UntestedPackageManager(pm) => serde_json::json!({ "kind": "untested_package_manager", "packageManager": format!("{pm:?}").to_lowercase() }),
            Warning::InstallScripts { file, scripts } => serde_json::json!({ "kind": "install_scripts", "file": file, "scripts": scripts }),
            Warning::UrlDependencies { file, packages } => serde_json::json!({ "kind": "url_dependencies", "file": file, "packages": packages }),
            Warning::AutoRunFiles { files } => serde_json::json!({ "kind": "auto_run_files", "files": files }),
        };
        v["text"] = Value::String(self.to_string());
        v
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::NestedSolution { found_in } => write!(f, "this folder is inside another solution ({})", found_in.display()),
            Warning::LongPath { length, limit } => {
                write!(f, "paths of about {length} characters once installed; the classic Windows limit is {limit}")
            }
            Warning::UntestedPackageManager(pm) => write!(f, "{pm:?} was not tested with this generator"),
            Warning::InstallScripts { file, scripts } => write!(
                f,
                "{file} has scripts that run by themselves when it is installed ({}): read them before installing",
                scripts.join(", ")
            ),
            Warning::UrlDependencies { file, packages } => write!(
                f,
                "{file} gets {} from a URL, a git repository or a local path, not from the package registry",
                packages.join(", ")
            ),
            Warning::AutoRunFiles { files } => {
                let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
                write!(
                    f,
                    "{} file(s) that tools run or obey by themselves: {}{}: read them before opening the project in an editor or installing",
                    files.len(),
                    shown.join(", "),
                    if files.len() > shown.len() { ", ..." } else { "" }
                )
            }
        }
    }
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
    templates: &dyn TemplateSource,
    answers: &Answers,
    options: PlanOptions,
) -> Result<Plan> {
    plan_impl(manifest, templates, answers, options, false)
}

/// The same plan without looking at any folder on disk: the location need not exist, the folder it would make is not
/// looked for, and no nested solution is searched for. For checking a template (`crate::check`), where the answer is
/// only "would this render".
pub(crate) fn plan_dry(
    manifest: &Manifest,
    templates: &dyn TemplateSource,
    answers: &Answers,
    options: PlanOptions,
) -> Result<Plan> {
    plan_impl(manifest, templates, answers, options, true)
}

fn plan_impl(
    manifest: &Manifest,
    templates: &dyn TemplateSource,
    answers: &Answers,
    mut options: PlanOptions,
    dry: bool,
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
    if !dry && !options.location.is_dir() {
        return Err(ScaffoldError::InvalidPath {
            path: options.location.display().to_string(),
            reason: "the location does not exist".into(),
        });
    }
    let target = options.location.join(&folder_name);
    if !dry && target.exists() {
        return Err(ScaffoldError::FolderExists(target));
    }

    // ---- render everything
    let (files, _) = render_stage(
        manifest,
        templates,
        &answers,
        options.guids.as_mut(),
        options.secrets.as_mut(),
        options.limits,
    )?;

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
    if let Some(marker) = manifest.nested_marker.as_ref().filter(|_| !dry) {
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

    // TODO(forge-analysis): these three checks (install scripts, URL dependencies, auto-run files) are the cheap first look
    // and stay. The analysis engine is meant to go beyond them: read the code the template writes. It reads a folder on
    // disk, so it plugs in after `apply()` has written the project and before the host runs `install`, not here (a plan
    // writes nothing). That call belongs to the host, which holds the user's consent; this crate should only return the
    // report's summary with the plan's result once the engine is a dependency. Not wired until it is finished, tested and
    // confirmed to find what `docs/integration-notes.md` lists.
    for path in files.paths().filter(|p| *p == "package.json" || p.ends_with("/package.json")) {
        if let Some(bytes) = files.get(path) {
            warnings.extend(package_json_warnings(path, bytes, None));
        }
    }
    let auto_run: Vec<String> = files.paths().filter(|p| is_auto_run_path(p)).map(str::to_string).collect();
    if !auto_run.is_empty() {
        warnings.push(Warning::AutoRunFiles { files: auto_run });
    }

    Ok(Plan {
        target,
        folder_name,
        files,
        warnings,
        post,
    })
}

/// Whether a path is a file that tools run or obey by themselves (see [`Warning::AutoRunFiles`]).
pub(crate) fn is_auto_run_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let inside = |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    inside(".husky")
        || inside(".devcontainer")
        || inside(".yarn/plugins")
        || inside(".yarn/releases")
        || lower == ".vscode/tasks.json"
        || lower.ends_with("/.vscode/tasks.json")
        || matches!(
            name,
            ".devcontainer.json" | ".envrc" | ".npmrc" | ".yarnrc" | ".yarnrc.yml" | ".pnpmfile.cjs" | ".pnpmfile.js" | ".pnpmfile.mjs"
        )
}

/// Scripts the package manager runs by itself when it installs a project.
const INSTALL_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall", "prepare", "preprepare", "postprepare"];

/// What installing would run or fetch, found in one `package.json` (`new`). With `previous` (the file as it was before
/// an add merged into it) only what the new text adds is reported: a script or a dependency the solution already had is
/// the solution's own. The engine runs nothing, so this only tells the host; reading a script is not the same as
/// judging it, which is for a person (or a checking tool) to do.
pub(crate) fn package_json_warnings(path: &str, new: &[u8], previous: Option<&[u8]>) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let Ok(package) = serde_json::from_slice::<Value>(new) else { return warnings };
    let before: Option<Value> = previous.and_then(|p| serde_json::from_slice(p).ok());

    let scripts: Vec<String> = package
        .get("scripts")
        .and_then(Value::as_object)
        .map(|s| {
            INSTALL_SCRIPTS
                .iter()
                .filter(|name| {
                    s.get(**name).is_some()
                        && before.as_ref().and_then(|b| b.pointer(&format!("/scripts/{name}"))) != s.get(**name)
                })
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default();
    if !scripts.is_empty() {
        warnings.push(Warning::InstallScripts { file: path.to_string(), scripts });
    }

    let mut packages: Vec<String> = Vec::new();
    for section in ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"] {
        for (name, spec) in package.get(section).and_then(Value::as_object).into_iter().flatten() {
            let had = before.as_ref().and_then(|b| b.pointer(&format!("/{section}/{}", name.replace('~', "~0").replace('/', "~1"))));
            if spec.as_str().is_some_and(is_non_registry_spec) && had != Some(spec) && !packages.contains(name) {
                packages.push(name.clone());
            }
        }
    }
    if !packages.is_empty() {
        warnings.push(Warning::UrlDependencies { file: path.to_string(), packages });
    }
    warnings
}

/// A dependency that is not a version or range from the registry: a URL, a git repository, a local path,
/// or GitHub's `user/repo` shorthand. (`npm:` aliases and `workspace:` are left alone.)
fn is_non_registry_spec(spec: &str) -> bool {
    let spec = spec.trim();
    const PREFIXES: &[&str] = &[
        "http:", "https:", "git+", "git:", "git@", "ssh:", "github:", "gitlab:", "bitbucket:", "gist:", "file:", "link:",
    ];
    if PREFIXES.iter().any(|p| spec.starts_with(p)) {
        return true;
    }
    // `user/repo` or `user/repo#ref`
    let repo = spec.split('#').next().unwrap_or(spec);
    let mut parts = repo.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(user), Some(name), None) => {
            let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            plain(user) && plain(name)
        }
        _ => false,
    }
}

/// Renders a manifest into an in-memory stage: tokens, `include`, `repeat`,
/// `parts` and `merge`. Writes nothing and looks at no folder on disk, so the
/// add mode can reuse it. Also returns the solution-level tokens.
pub(crate) fn render_stage(
    manifest: &Manifest,
    templates: &dyn TemplateSource,
    answers: &Answers,
    guids: &mut dyn GuidSource,
    secrets: &mut dyn SecretSource,
    limits: Limits,
) -> Result<(StagedFs, TokenMap)> {
    // ---- tokens
    let mut base = TokenMap::new();
    for p in &manifest.prompts {
        // a prompt whose condition failed has no answer and defines no token
        let Some(value) = answers.get(&p.key) else { continue };
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
        base.insert_guid(g, guids)?;
    }
    for s in &manifest.secrets {
        base.insert(s, secrets.next_secret())?;
    }

    let mut files = StagedFs::with_limits(limits);

    // ---- include: folders copied once
    for inc in manifest.include.iter().filter(|i| when_holds(&i.when, answers)) {
        copy_folder(
            &mut files,
            templates,
            &inc.folder,
            inc.to.as_deref(),
            &inc.rename,
            &base,
        )?;
    }

    // ---- repeat: folders per list item. Remember each item's tokens for merges.
    let mut item_tokens: Vec<(String, Vec<TokenMap>)> = Vec::new(); // (over, per-item maps)
    for r in manifest.repeat.iter().filter(|r| when_holds(&r.when, answers)) {
        let items = list_items(answers, &r.over);
        let mut maps = Vec::with_capacity(items.len());
        for item in &items {
            let mut tokens = base.clone();
            tokens.insert_name(&r.item, item)?;
            for g in &r.guids {
                tokens.insert_guid(g, guids)?;
            }
            copy_folder(&mut files, templates, &r.folder, None, &r.rename, &tokens)?;
            maps.push(tokens);
        }
        item_tokens.push((r.over.clone(), maps));
    }

    // ---- parts: assembled text files
    let mut assembled: Vec<(String, String)> = Vec::new();
    for part in manifest.parts.iter().filter(|p| when_holds(&p.when, answers)) {
        let target_path = render(&part.target, &base, "part target")?;
        let text = read_template_text(templates, &part.source)?;
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
    for m in manifest.merge.iter().filter(|m| when_holds(&m.when, answers)) {
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
                    let text = read_template_text(templates, vf)?;
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

    Ok((files, base))
}

/// Writes the plan into its new folder. Fails if the folder exists, and removes
/// the folder again if writing fails halfway.
pub fn apply(plan: &Plan, on_progress: impl FnMut(WriteProgress)) -> Result<WriteOutcome> {
    plan.files.write_to(&plan.target, on_progress)
}

// ---------------------------------------------------------------- answers

/// Applies defaults and checks every answer against its prompt.
fn resolve_answers(manifest: &Manifest, given: &Answers) -> Result<Answers> {
    resolve_answers_for(manifest, given, None)
}

/// Like [`resolve_answers`], but only the prompts named in `only` are asked for
/// and checked (the add mode needs the web part list, not the solution name).
/// Answers to prompts that exist in the manifest are always accepted.
pub(crate) fn resolve_answers_for(
    manifest: &Manifest,
    given: &Answers,
    only: Option<&HashSet<String>>,
) -> Result<Answers> {
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
        if only.is_some_and(|keys| !keys.contains(&p.key)) {
            continue;
        }
        // Not asked: no answer, nothing to check. An answer given for it anyway (a form that kept an old value) is ignored.
        // `out` holds the earlier answers, which is all a condition may look at.
        if !when_holds(&p.when, &out) {
            continue;
        }
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
            (Some(NameRule::PackageName), Value::String(s)) => {
                issues.extend(retarget(validate_package_name(s), &p.key));
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

pub(crate) fn list_items(answers: &Answers, key: &str) -> Vec<String> {
    answers
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

// --------------------------------------------------------------- templates

/// Copies a template folder into the stage, rendering paths and text contents.
pub(crate) fn copy_folder(
    files: &mut StagedFs,
    templates: &dyn TemplateSource,
    folder: &str,
    to: Option<&str>,
    rename: &BTreeMap<String, String>,
    tokens: &TokenMap,
) -> Result<()> {
    let folder = normalize_rel(folder)?;
    for rel in templates.list_files(&folder)? {
        let source = format!("{folder}/{rel}");
        let mut dest = render(&rel, tokens, &format!("path {source}"))?;
        // Only the file's own name is renamed, wherever in the folder it is.
        if !rename.is_empty() {
            let (dir, name) = match dest.rsplit_once('/') {
                Some((dir, name)) => (Some(dir), name),
                None => (None, dest.as_str()),
            };
            if let Some(new) = rename.get(name) {
                dest = match dir {
                    Some(dir) => format!("{dir}/{new}"),
                    None => new.clone(),
                };
            }
        }
        if let Some(to) = to {
            dest = format!("{}/{dest}", to.trim_matches('/'));
        }
        let bytes = templates.read_file(&source)?;
        // Text is rendered; anything that is not valid UTF-8 is copied as is.
        let bytes = match String::from_utf8(bytes) {
            Ok(text) => render(&text, tokens, &source)?.into_bytes(),
            Err(e) => e.into_bytes(),
        };
        files.insert(&dest, bytes)?;
    }
    Ok(())
}

pub(crate) fn read_template_text(templates: &dyn TemplateSource, rel: &str) -> Result<String> {
    templates.read_text(&normalize_rel(rel)?)
}

// ------------------------------------------------------------------- merge

/// Renders every string inside a JSON value. Types are preserved: a string
/// stays a string (use `valueFile` when a token must become a number).
pub(crate) fn render_json(value: &Value, tokens: &TokenMap, context: &str) -> Result<Value> {
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

pub(crate) fn unreachable_manifest(file: &str) -> Result<Value> {
    Err(ScaffoldError::Merge {
        file: file.to_string(),
        reason: "the manifest gives neither 'value' nor 'valueFile'".into(),
    })
}

pub(crate) fn apply_merge(
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

    let segments: Vec<&str> = path.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(fail(format!("'{path}' is not a valid dotted path")));
    }
    let mut doc: Value = match serde_json::from_str(text) {
        Ok(doc) => doc,
        Err(strict) => {
            // Comments or trailing commas: edit the text itself, which keeps them.
            let edited = crate::jsonc::merge_path(text, &segments, strategy, &value)
                .map_err(|e| fail(format!("not valid JSON ({strict}); as JSON with comments: {e}")))?;
            return files.replace(file, edited.into_bytes());
        }
    };
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

    let out = serialize_like(text, &doc).map_err(fail)?;
    files.replace(file, out.into_bytes())
}

/// The whitespace one nesting level is indented with in `text`: a tab, or the
/// spaces of the first indented line. Two spaces when nothing is indented.
pub(crate) fn detect_indent(text: &str) -> String {
    for line in text.lines() {
        let lead: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if !lead.is_empty() && lead.len() < line.len() {
            return if lead.starts_with('\t') { "\t".to_string() } else { lead };
        }
    }
    "  ".to_string()
}

/// Writes `doc` the way `original` was written: its indent, its line endings
/// and whether it ends with a newline, so editing a file does not rewrite all of
/// it. Key order is kept. Comments cannot be kept (the file must be plain JSON).
pub(crate) fn serialize_like(original: &str, doc: &Value) -> std::result::Result<String, String> {
    let indent = detect_indent(original);
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(doc, &mut serializer).map_err(|e| e.to_string())?;
    let mut out = String::from_utf8(buf).map_err(|e| e.to_string())?;
    // A single-line original (a fixture, say) has no line endings to copy.
    if original.ends_with('\n') || !original.contains('\n') {
        out.push('\n');
    }
    if original.contains("\r\n") {
        out = out.replace('\n', "\r\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::secrets::SequentialSecrets;
    use crate::source::DirSource;
    use std::fs;
    use std::path::Path;
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
      "version": "1.23.2",
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

    /// Builds a pack from every top-level folder of a template directory.
    /// With `shared`, every file goes into `blobs` and is referenced by hash.
    fn pack_from_dir(t: &TempDir, manifest: &str, shared: bool) -> crate::pack::Pack {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let src = DirSource::new(&t.0);
        let mut entries = Vec::new();
        let mut blobs = serde_json::Map::new();
        for top in fs::read_dir(&t.0).unwrap() {
            let top = top.unwrap().file_name().to_string_lossy().into_owned();
            for rel in src.list_files(&top).unwrap() {
                let path = format!("{top}/{rel}");
                let bytes = src.read_file(&path).unwrap();
                let (encoding, content) = match String::from_utf8(bytes.clone()) {
                    Ok(text) => ("utf8", text),
                    Err(_) => ("base64", STANDARD.encode(&bytes)),
                };
                if shared {
                    let key = crate::index::sha256_hex(&bytes);
                    blobs.insert(key.clone(), json!({ "encoding": encoding, "content": content }));
                    entries.push(json!({ "type": "file", "path": path, "blob": key }));
                } else {
                    entries.push(json!({ "type": "file", "path": path, "encoding": encoding, "content": content }));
                }
            }
        }
        let m: Value = serde_json::from_str(manifest).unwrap();
        let pack = json!({ "schema": 1, "generator": m["id"], "version": m["version"],
                           "revision": 1, "manifest": m, "blobs": blobs, "entries": entries });
        crate::pack::Pack::from_json(&pack.to_string()).unwrap()
    }

    fn assert_same_plan(t: &TempDir, pack: &crate::pack::Pack) {
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld", "News"], "install": true }));
        let from_dir = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
        let from_pack = plan(pack.manifest(), pack, &a, opts(&loc.0)).unwrap();

        let dir_paths: Vec<&str> = from_dir.files.paths().collect();
        let pack_paths: Vec<&str> = from_pack.files.paths().collect();
        assert_eq!(dir_paths, pack_paths);
        assert!(dir_paths.len() > 6);
        for p in dir_paths {
            assert_eq!(from_dir.files.get(p), from_pack.files.get(p), "content differs: {p}");
        }
        assert_eq!(from_pack.files.get("logo.bin").unwrap(), &[0xff, 0xfe, 0x00, 0x7b, 0x5f]);
        assert_eq!(from_dir.post, from_pack.post);
    }

    #[test]
    fn a_pack_gives_the_same_plan_as_the_folder_it_was_made_from() {
        let t = spfx_templates();
        t.write("solution/notes.txt", "{__solutionName__}\n");
        fs::write(t.0.join("solution/logo.bin"), [0xff, 0xfe, 0x00, 0x7b, 0x5f]).unwrap();
        assert_same_plan(&t, &pack_from_dir(&t, SPFX, false));
    }

    #[test]
    fn a_pack_with_shared_blobs_gives_the_same_plan() {
        let t = spfx_templates();
        t.write("solution/notes.txt", "{__solutionName__}\n");
        fs::write(t.0.join("solution/logo.bin"), [0xff, 0xfe, 0x00, 0x7b, 0x5f]).unwrap();
        // The same bytes under two names: stored once, used twice.
        fs::write(t.0.join("solution/logo-copy.bin"), [0xff, 0xfe, 0x00, 0x7b, 0x5f]).unwrap();
        let pack = pack_from_dir(&t, SPFX, true);
        assert!(pack.stored_bytes() < pack.total_bytes(), "the duplicate must be stored once");
        assert_same_plan(&t, &pack);
    }

    #[test]
    fn spfx_like_plan_renders_repeats_merges_and_guids() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld", "News"] }));
        let plan = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();

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
        let one = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
        let two = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
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
        let p = plan(&m, &DirSource::new(&t.0), &a, o).unwrap();
        assert_eq!(p.post.len(), 1);
        assert_eq!(p.post[0].action, PostAction::Install);
        assert_eq!(p.post[0].package_manager, Some(PackageManager::Pnpm));
        assert_eq!(p.post[0].cwd, p.target);
        assert!(p.warnings.contains(&Warning::UntestedPackageManager(PackageManager::Pnpm)));
        // nothing was written
        assert!(!p.target.exists());

        // default package manager is the tested one: no warning
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
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

        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&outer.0.join("inner"))).unwrap();
        assert!(p.warnings.contains(&Warning::NestedSolution { found_in: outer.0.clone() }));

        // A deep location triggers the long-path warning, only when install is on.
        let deep = outer.0.join("a".repeat(100)).join("b".repeat(30));
        fs::create_dir_all(&deep).unwrap();
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&deep)).unwrap();
        assert!(p.warnings.iter().any(|w| matches!(w, Warning::LongPath { .. })));
        let no_install = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"] }));
        let p = plan(&m, &DirSource::new(&t.0), &no_install, opts(&deep)).unwrap();
        assert!(!p.warnings.iter().any(|w| matches!(w, Warning::LongPath { .. })));
    }

    #[test]
    fn bad_answers_and_bad_names_are_reported_before_anything_happens() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();

        let err = plan(&m, &DirSource::new(&t.0), &answers(json!({ "webparts": ["A"] })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("'solutionName' is required"), "{err}");

        let err = plan(&m, &DirSource::new(&t.0), &answers(json!({ "solutionName": "x", "webparts": ["A"], "nope": 1 })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("unknown answer"), "{err}");

        let err = plan(&m, &DirSource::new(&t.0), &answers(json!({ "solutionName": "My Solution", "webparts": ["1bad", "1bad"] })), opts(&loc.0)).unwrap_err();
        match err {
            ScaffoldError::Validation(issues) => {
                assert!(issues.iter().any(|i| i.field == "solutionName"));
                assert!(issues.iter().any(|i| i.field.starts_with("webparts[")));
            }
            other => panic!("expected Validation, got {other}"),
        }

        let err = plan(&m, &DirSource::new(&t.0), &answers(json!({ "solutionName": "ok", "webparts": [] })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("at least one web part"), "{err}");
    }

    #[test]
    fn existing_folder_and_missing_location_are_errors() {
        let t = spfx_templates();
        let loc = TempDir::new();
        fs::create_dir(loc.0.join("my-solution")).unwrap();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["A"] }));
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err();
        assert!(matches!(err, ScaffoldError::FolderExists(_)));
        assert!(err.to_string().contains("please choose a different name"));

        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0.join("missing"))).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn unknown_tokens_in_templates_are_an_error_with_the_file_named() {
        let t = spfx_templates();
        t.write("solution/oops.txt", "{__doesNotExist__}");
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "x", "webparts": ["A"] }));
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("doesNotExist") && err.contains("solution/oops.txt"), "{err}");
    }

    #[test]
    fn merging_into_a_missing_or_broken_file_is_an_error() {
        let t = spfx_templates();
        t.write("solution/config/config.json", "{ not json");
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "x", "webparts": ["A"] }));
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("config/config.json") && err.contains("not valid JSON"), "{err}");

        fs::remove_file(t.0.join("solution/config/config.json")).unwrap();
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("not part of the output"), "{err}");
    }

    #[test]
    fn apply_writes_the_plan_and_refuses_an_existing_folder() {
        let t = spfx_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(SPFX).unwrap();
        let a = answers(json!({ "solutionName": "my-solution", "webparts": ["HelloWorld"] }));
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();

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
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("symbolic links"), "{err}");
    }

    // ----------------------------------------------------------- full stack

    const FULLSTACK: &str = r#"{
      "id": "fullstack",
      "version": "1.23.2",
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
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();

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
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
        assert_eq!(p.files.get_text("docker-compose.yml").unwrap(), "services:\n  db:\n    image: mssql\n");
        assert!(!p.files.contains(".zed/workflows/flow-task-chain.flow.json"));
        let actions: Vec<_> = p.post.iter().map(|s| s.action).collect();
        assert_eq!(actions, vec![PostAction::ComposeUp, PostAction::Install]);
        assert_eq!(p.post[1].cwd, p.target.join("client"));

        // native: no compose file at all
        let a = answers(json!({ "projectName": "task-chain", "preset": "native" }));
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
        assert!(!p.files.contains("docker-compose.yml"));

        // a choice answer outside its options is rejected
        let a = answers(json!({ "projectName": "task-chain", "db": "oracle" }));
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
        assert!(err.contains("'db' must be one of"), "{err}");
    }

    #[test]
    fn sensitive_values_are_plain_tokens_and_never_printed() {
        let t = fullstack_templates();
        t.write("base/secret.txt", "{__dbPassword__}");
        let loc = TempDir::new();
        let m = Manifest::from_json(FULLSTACK).unwrap();
        let a = answers(json!({ "projectName": "task-chain", "dbPassword": "hunter2-Hunter2" }));
        let p = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap();
        assert_eq!(p.files.get_text("secret.txt").unwrap(), "hunter2-Hunter2");

        // Debug output of the plan has no file contents
        let debug = format!("{p:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(!debug.contains("Aa11x"), "{debug}");

        // no derived casings for sensitive tokens
        t.write("base/secret.txt", "{__dbPasswordPascal__}");
        let err = plan(&m, &DirSource::new(&t.0), &a, opts(&loc.0)).unwrap_err().to_string();
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

    // ------------------------------------------------- rename and package-name

    const VITE_LIKE: &str = r#"{
      "id": "vite-demo", "version": "1.0.0", "folderFrom": "projectName",
      "prompts": [ { "key": "projectName", "type": "text", "rule": "package-name" } ],
      "include": [
        { "folder": "app", "rename": { "_gitignore": ".gitignore" } },
        { "folder": "extra", "to": "sub", "rename": { "_npmrc": ".npmrc" } }
      ]
    }"#;

    fn vite_like_templates() -> TempDir {
        let t = TempDir::new();
        t.write("app/_gitignore", "node_modules\n");
        t.write("app/src/_gitignore", "nested\n");
        t.write("app/_gitignore.bak", "not renamed: only a whole file name is\n");
        t.write("app/package.json", "{ \"name\": \"{__projectName__}\" }\n");
        t.write("extra/_npmrc", "registry=x\n");
        t.write("extra/deep/_npmrc", "deep\n");
        t
    }

    #[test]
    fn rename_changes_a_files_own_name_wherever_it_is_and_under_a_destination_prefix() {
        let t = vite_like_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(VITE_LIKE).unwrap();
        let p = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "my.app_1" })), opts(&loc.0)).unwrap();
        let got: Vec<&str> = p.files.paths().collect();
        assert_eq!(
            got,
            [".gitignore", "_gitignore.bak", "package.json", "src/.gitignore", "sub/.npmrc", "sub/deep/.npmrc"],
            "{got:?}"
        );
        assert_eq!(p.files.get(".gitignore").unwrap(), b"node_modules\n");
        assert_eq!(p.files.get("src/.gitignore").unwrap(), b"nested\n");
        assert_eq!(p.files.get("package.json").unwrap(), b"{ \"name\": \"my.app_1\" }\n");
    }

    #[test]
    fn rename_also_works_on_a_repeated_folder_and_is_written_to_disk() {
        let t = TempDir::new();
        t.write("part/_dotfile", "{__itemKebab__}\n");
        let m = Manifest::from_json(
            r#"{ "id": "r", "version": "1.0.0", "folderFrom": "n",
              "prompts": [ { "key": "n", "type": "text" }, { "key": "items", "type": "list" } ],
              "repeat": [ { "over": "items", "as": "item", "folder": "part", "rename": { "_dotfile": ".dotfile" } } ] }"#,
        )
        .unwrap();
        let loc = TempDir::new();
        let p = plan(&m, &DirSource::new(&t.0), &answers(json!({ "n": "proj", "items": ["One"] })), opts(&loc.0)).unwrap();
        assert_eq!(p.files.paths().collect::<Vec<_>>(), [".dotfile"]);
        apply(&p, |_| {}).unwrap();
        assert_eq!(fs::read_to_string(loc.0.join("proj/.dotfile")).unwrap(), "one\n");
    }

    #[test]
    fn the_package_name_rule_is_checked_when_planning() {
        let t = vite_like_templates();
        let loc = TempDir::new();
        let m = Manifest::from_json(VITE_LIKE).unwrap();
        let src = DirSource::new(&t.0);
        for ok in ["vite-app", "my.app", "123", "a_b"] {
            assert!(plan(&m, &src, &answers(json!({ "projectName": ok })), opts(&loc.0)).is_ok(), "{ok}");
        }
        let err = plan(&m, &src, &answers(json!({ "projectName": "My App" })), opts(&loc.0)).unwrap_err();
        match err {
            ScaffoldError::Validation(issues) => {
                assert!(issues.iter().all(|i| i.field == "projectName"), "{issues:?}");
                assert!(issues.iter().any(|i| i.message.contains("No spaces")));
            }
            other => panic!("{other}"),
        }
        // a scoped name is a good package name but cannot be the folder: that is said, not guessed
        let err = plan(&m, &src, &answers(json!({ "projectName": "@scope/app" })), opts(&loc.0)).unwrap_err();
        assert!(err.to_string().contains("single name"), "{err}");
    }

    #[test]
    fn package_json_warnings_report_only_what_is_new_against_the_previous_file() {
        let before = br#"{ "scripts": { "postinstall": "node a.js" }, "dependencies": { "a": "git+https://x/a.git", "b": "1.0.0" } }"#;
        let after = br#"{ "scripts": { "postinstall": "node a.js", "prepare": "husky" },
                          "dependencies": { "a": "git+https://x/a.git", "b": "https://x/b.tgz", "c": "^2.0.0", "d": "owner/repo" } }"#;
        let w = package_json_warnings("package.json", after, Some(before));
        assert_eq!(
            w,
            vec![
                Warning::InstallScripts { file: "package.json".into(), scripts: vec!["prepare".into()] },
                Warning::UrlDependencies { file: "package.json".into(), packages: vec!["b".into(), "d".into()] },
            ]
        );
        // with no previous file, everything counts
        let all = package_json_warnings("package.json", after, None);
        assert_eq!(all.len(), 2);
        assert!(matches!(&all[0], Warning::InstallScripts { scripts, .. } if scripts == &["postinstall".to_string(), "prepare".to_string()]));
        // a file that is not JSON, or has nothing to say, gives nothing
        assert!(package_json_warnings("package.json", b"not json", None).is_empty());
        assert!(package_json_warnings("package.json", br#"{ "dependencies": { "a": "^1.0.0" } }"#, None).is_empty());
        // the way a warning reads
        assert!(all[1].to_string().contains("from a URL, a git repository or a local path"));
    }

    #[test]
    fn files_that_tools_run_by_themselves_are_recognised_and_ordinary_dot_files_are_not() {
        for yes in [
            ".husky/pre-commit",
            "app/.husky/_/husky.sh",
            ".vscode/tasks.json",
            "app/.vscode/tasks.json",
            ".devcontainer/devcontainer.json",
            ".devcontainer.json",
            ".envrc",
            ".npmrc",
            "sub/.NPMRC",
            ".yarnrc",
            ".yarnrc.yml",
            ".pnpmfile.cjs",
            ".yarn/plugins/@yarnpkg/plugin-x.cjs",
            ".yarn/releases/yarn-4.0.0.cjs",
        ] {
            assert!(is_auto_run_path(yes), "{yes} was not recognised");
        }
        for no in [
            ".vscode/launch.json",
            ".vscode/settings.json",
            ".github/workflows/ci.yml",
            ".gitignore",
            ".npmignore",
            ".eslintrc.json",
            "src/npmrc.ts",
            "docs/husky.md",
            "tasks.json",
            ".yarn/cache/x.zip",
        ] {
            assert!(!is_auto_run_path(no), "{no} was recognised");
        }
    }

    // ------------------------------------------------- conditional prompts

    const COND: &str = r#"{
      "id": "cond", "version": "1.0.0", "folderFrom": "projectName",
      "prompts": [
        { "key": "projectName", "type": "text" },
        { "key": "database", "type": "bool", "default": false },
        { "key": "dbKind", "type": "choice", "options": ["postgres", "mysql"], "when": { "key": "database", "equals": true } },
        { "key": "dbPort", "type": "text", "default": "3306", "rule": "package-name",
          "when": { "all": [ { "key": "database", "equals": true }, { "key": "dbKind", "equals": "mysql" } ] } },
        { "key": "pages", "type": "list", "when": { "key": "database", "equals": false } }
      ],
      "include": [ { "folder": "base" }, { "folder": "db", "when": { "key": "database", "equals": true } } ],
      "repeat": [ { "over": "pages", "as": "page", "folder": "page" } ]
    }"#;

    fn cond_templates() -> TempDir {
        let t = TempDir::new();
        t.write("base/readme.txt", "{__projectName__}\n");
        t.write("db/db.txt", "kind {__dbKind__}\n");
        t.write("page/{__pageKebab__}.txt", "page {__page__}\n");
        t
    }

    fn cond_paths(t: &TempDir, a: Value) -> Result<Vec<String>> {
        let loc = TempDir::new();
        let m = Manifest::from_json(COND).unwrap();
        let p = plan(&m, &DirSource::new(&t.0), &answers(a), opts(&loc.0))?;
        Ok(p.files.paths().map(str::to_string).collect())
    }

    #[test]
    fn a_prompt_is_asked_only_when_its_condition_holds() {
        let t = cond_templates();
        // database is off: dbKind is not asked (it has no default and is not required), pages is asked and may be empty
        assert_eq!(cond_paths(&t, json!({ "projectName": "p" })).unwrap(), ["readme.txt"]);
        assert_eq!(cond_paths(&t, json!({ "projectName": "p", "pages": ["About Us"] })).unwrap(), ["about-us.txt", "readme.txt"]);

        // database is on: dbKind is now asked, and it is required
        let err = cond_paths(&t, json!({ "projectName": "p", "database": true })).unwrap_err().to_string();
        assert!(err.contains("'dbKind' is required"), "{err}");
        // pages is not asked any more, so its answer is ignored and it repeats nothing
        let paths = cond_paths(&t, json!({ "projectName": "p", "database": true, "dbKind": "postgres", "pages": ["Ignored"] })).unwrap();
        assert_eq!(paths, ["db.txt", "readme.txt"]);

        // dbPort is asked only for mysql, and then it uses its default
        assert!(cond_paths(&t, json!({ "projectName": "p", "database": true, "dbKind": "mysql" })).is_ok());
    }

    #[test]
    fn an_answer_given_for_a_prompt_that_is_not_asked_is_ignored_and_not_checked() {
        let t = cond_templates();
        // a form kept old values for dbKind and dbPort (not valid ones) after the user turned the database off
        let ok = cond_paths(&t, json!({ "projectName": "p", "dbKind": "oracle", "dbPort": "Not A Valid Name" }));
        assert_eq!(ok.unwrap(), ["readme.txt"]);

        // asked again, the same answer is checked: dbPort has the package-name rule
        let err = cond_paths(&t, json!({ "projectName": "p", "database": true, "dbKind": "mysql", "dbPort": "Not A Valid Name" })).unwrap_err();
        match err {
            ScaffoldError::Validation(issues) => assert!(issues.iter().all(|i| i.field == "dbPort") && !issues.is_empty(), "{issues:?}"),
            other => panic!("{other}"),
        }
        // and an answer for a prompt the manifest does not have at all is still an error
        assert!(cond_paths(&t, json!({ "projectName": "p", "nope": 1 })).unwrap_err().to_string().contains("unknown answer"));
    }

    #[test]
    fn a_prompt_that_is_not_asked_defines_no_token() {
        let t = cond_templates();
        t.write("base/oops.txt", "{__dbKind__}\n"); // always copied, but dbKind is not always asked
        let err = cond_paths(&t, json!({ "projectName": "p" })).unwrap_err();
        assert!(matches!(err, ScaffoldError::UnknownToken { ref token, .. } if token == "dbKind"), "{err}");
        // asked, the token exists
        assert!(cond_paths(&t, json!({ "projectName": "p", "database": true, "dbKind": "mysql" })).is_ok());
    }
}
