//! Making a template from an existing project: "save this project as a template".
//!
//! [`create_template`] reads a project folder and turns the texts the user names (the project name, a company name, a
//! namespace) into the engine's tokens, in file contents and in file and folder names. The result is a generator: a
//! `manifest.json` with one prompt per named text, and the project's files below `template/`.
//!
//! The user names the texts. The engine does not guess which words in a project are "the name": a wrong guess would quietly
//! damage files. Each named text becomes the token `{__key__}` and, where it differs, its derived forms: `{__keyPascal__}`,
//! `{__keyCamel__}`, `{__keyKebab__}`, `{__keyAllCaps__}`. A text that does not appear in the project is an error, and the
//! draft says how many times each one was replaced and in how many files.
//!
//! What is left out, and why (each one is listed in the draft with its reason):
//!
//! * dependency and build folders (`node_modules`, `dist`, `build`, `target`, `bin`, `obj`, `.git`, ...), which a new
//!   project regenerates;
//! * files that may hold secrets (`.env`, `.env.*`, `*.pem`, `*.key`, `*.pfx`, `id_rsa*`), which must never end up in a template;
//! * lock files, which pin one project's resolved versions;
//! * links (symbolic links, junctions), which a template cannot hold;
//! * anything else the caller names in `exclude`.
//!
//! `.gitignore`, `.gitattributes` and `.gitmodules` are stored as `_gitignore`, ... and written back under their real
//! names (the manifest's `rename`), so that a git repository that holds the template does not obey them.
//!
//! **A draft is verified before it is returned.** The engine scaffolds the draft with the original texts as the answers and
//! compares the result with the project it came from, file by file and byte for byte. A draft that does not reproduce the
//! project is an error, not a template.

use crate::check::{check_with_manifest, CheckOptions};
use crate::error::{Result, ScaffoldError};
use crate::manifest::Manifest;
use crate::plan::{plan_dry, Answers, PlanOptions};
use crate::secrets::SequentialSecrets;
use crate::source::TemplateSource;
use crate::stage::{Limits, StagedFs, WriteOutcome};
use crate::tokens::{to_all_caps, to_camel, to_kebab, to_pascal, SequentialGuids};
use crate::validate::{validate_package_name, validate_solution_name};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A text in the project that becomes a prompt of the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// The prompt's key, which is also the token's name: letters and digits, starting with a lowercase letter
    /// (`projectName`).
    pub key: String,
    /// The text as it is in the project (`my-app`). Case matters.
    pub text: String,
    /// What the form asks (`Project name`). Defaults to the key.
    pub label: Option<String>,
}

impl Replacement {
    pub fn new(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self { key: key.into(), text: text.into(), label: None }
    }
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// The generator's id (`my-template`) and version (`1.0.0`, three numbers).
    pub id: String,
    pub version: String,
    /// The texts to turn into prompts. The first is the name of the project's folder (`folderFrom`), and at least one is
    /// needed.
    pub replacements: Vec<Replacement>,
    /// More names to leave out, wherever they appear in a path (a folder or a file name, not a pattern).
    pub exclude: Vec<String>,
    /// How much the project may hold.
    pub limits: Limits,
}

impl CreateOptions {
    pub fn new(id: impl Into<String>, version: impl Into<String>, replacements: Vec<Replacement>) -> Self {
        Self { id: id.into(), version: version.into(), replacements, exclude: Vec::new(), limits: Limits::default() }
    }
}

/// Folders and files left out of every template.
const SKIP_NAMES: &[&str] = &[
    ".git", "node_modules", "dist", "build", "out", "target", "bin", "obj", ".next", ".nuxt", ".turbo", ".cache", "coverage",
    ".venv", "venv", "__pycache__", ".gradle", ".idea", ".vs", ".DS_Store", "Thumbs.db",
];
const LOCK_FILES: &[&str] = &["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "bun.lockb", "Cargo.lock", "composer.lock", "Gemfile.lock", "poetry.lock"];
/// Files stored under another name so that a repository holding the template does not obey them: (real name, stored name).
const GIT_FILES: &[(&str, &str)] = &[(".gitignore", "_gitignore"), (".gitattributes", "_gitattributes"), (".gitmodules", "_gitmodules")];

/// One thing left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// How often a text was replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacementCount {
    pub key: String,
    pub text: String,
    /// Times it was replaced, in file contents and in names.
    pub occurrences: usize,
    /// Files whose contents or name changed.
    pub files: usize,
}

/// A generator made from a project: in memory, until it is written.
#[derive(Debug, Clone)]
pub struct TemplateDraft {
    pub manifest_json: String,
    /// All files of the generator by path: `manifest.json` and everything below `template/`.
    files: BTreeMap<String, Vec<u8>>,
    pub skipped: Vec<Skipped>,
    pub counts: Vec<ReplacementCount>,
    /// Things worth knowing that are not mistakes: two texts that have a form in common (the first one named gets it).
    pub notes: Vec<String>,
}

impl TemplateDraft {
    /// The draft as a template source, to check it, plan from it, or release it.
    pub fn source(&self) -> DraftSource {
        DraftSource { files: self.files.clone() }
    }

    /// The path of every file of the generator, in order: `manifest.json`, then `template/...`.
    pub fn paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    /// How many files the template holds (not counting the manifest).
    pub fn file_count(&self) -> usize {
        self.files.len() - 1
    }

    /// Writes the generator into `target`, which must not exist yet (its parent must). Nothing is written if a path or a
    /// limit is refused, and the folder is removed again if a write fails.
    pub fn write_to(&self, target: &Path) -> Result<WriteOutcome> {
        let mut stage = StagedFs::new();
        for (path, bytes) in &self.files {
            stage.insert(path, bytes.clone())?;
        }
        stage.write_to(target, |_| {})
    }
}

/// A [`TemplateSource`] over the files of a draft.
#[derive(Debug, Clone)]
pub struct DraftSource {
    files: BTreeMap<String, Vec<u8>>,
}

impl TemplateSource for DraftSource {
    fn list_files(&self, folder: &str) -> Result<Vec<String>> {
        let folder = folder.trim_matches('/');
        let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
        Ok(self.files.keys().filter_map(|p| p.strip_prefix(&prefix)).map(str::to_string).collect())
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.files.get(path).cloned().ok_or_else(|| ScaffoldError::Template { path: path.to_string(), reason: "no such file".into() })
    }
}

fn bad(path: &str, reason: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Template { path: path.to_string(), reason: reason.into() }
}

/// What a replacement matches and what it becomes.
struct Forms(Vec<(String, String)>);

impl Forms {
    /// The raw text first, then its derived forms; a form equal to one already there is dropped.
    fn of(r: &Replacement) -> Forms {
        let mut out: Vec<(String, String)> = Vec::new();
        for (form, token) in [
            (r.text.clone(), r.key.clone()),
            (to_pascal(&r.text), format!("{}Pascal", r.key)),
            (to_camel(&r.text), format!("{}Camel", r.key)),
            (to_kebab(&r.text), format!("{}Kebab", r.key)),
            (to_all_caps(&r.text), format!("{}AllCaps", r.key)),
        ] {
            if !form.is_empty() && !out.iter().any(|(f, _)| *f == form) {
                out.push((form, format!("{{__{token}__}}")));
            }
        }
        Forms(out)
    }
}

/// Replaces every form of every replacement in `text`, the longest match first, in one pass (what was put in is never
/// searched again). Returns the new text and how many times each replacement (by index) was used.
fn replace_all(text: &str, forms: &[(String, String, usize)], counts: &mut [usize]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < text.len() {
        for (form, token, index) in forms {
            if text[i..].starts_with(form.as_str()) {
                out.push_str(token);
                counts[*index] += 1;
                i += form.len();
                continue 'outer;
            }
        }
        let c = text[i..].chars().next().expect("i is inside the text");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn is_secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".pfx")
        || lower.ends_with(".p12")
        || lower == "id_rsa"
        || lower.starts_with("id_rsa.")
        || lower == "id_ed25519"
        || lower.starts_with("id_ed25519.")
}

/// Whether text holds something the engine would read as one of its tokens.
fn has_token_shape(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("{__") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("__}") else { return false };
        let name = &after[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return true;
        }
        rest = &after[end + 3..];
    }
    false
}

struct Found {
    /// The path in the project, `/`-separated.
    path: String,
    bytes: Vec<u8>,
}

/// Reads the project: its files, minus what is left out.
fn read_project(root: &Path, options: &CreateOptions, skipped: &mut Vec<Skipped>) -> Result<Vec<Found>> {
    fn walk(dir: &Path, prefix: &str, options: &CreateOptions, out: &mut Vec<Found>, skipped: &mut Vec<Skipped>, total: &mut u64) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(|e| bad(prefix, format!("cannot read the folder: {e}")))?
            .collect::<std::io::Result<_>>()
            .map_err(|e| bad(prefix, e.to_string()))?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| bad(&rel, e.to_string()))?;
            let mut skip = |reason: &str| skipped.push(Skipped { path: rel.clone(), reason: reason.to_string() });

            if meta.file_type().is_symlink() {
                skip("a link: a template cannot hold one");
            } else if SKIP_NAMES.contains(&name.as_str()) || options.exclude.contains(&name) {
                skip("left out: a folder or file a new project regenerates, or that you excluded");
            } else if meta.is_dir() {
                walk(&entry.path(), &rel, options, out, skipped, total)?;
            } else if is_secret_name(&name) {
                skip("may hold a secret: it must not be put in a template");
            } else if LOCK_FILES.contains(&name.as_str()) {
                skip("a lock file pins one project's resolved versions");
            } else {
                if out.len() >= options.limits.max_files {
                    return Err(ScaffoldError::TooLarge(format!("the project has more than {} files", options.limits.max_files)));
                }
                if meta.len() > options.limits.max_file_bytes {
                    return Err(ScaffoldError::TooLarge(format!("'{rel}' is larger than {} bytes", options.limits.max_file_bytes)));
                }
                *total += meta.len();
                if *total > options.limits.max_total_bytes {
                    return Err(ScaffoldError::TooLarge(format!("the project is larger than {} bytes in all", options.limits.max_total_bytes)));
                }
                let bytes = fs::read(entry.path()).map_err(|e| bad(&rel, e.to_string()))?;
                out.push(Found { path: rel, bytes });
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    let mut total = 0u64;
    walk(root, "", options, &mut out, skipped, &mut total)?;
    Ok(out)
}

/// Makes a template draft from the project in `project`. See the module documentation.
pub fn create_template(project: &Path, options: &CreateOptions) -> Result<TemplateDraft> {
    if !project.is_dir() {
        return Err(bad(&project.display().to_string(), "the project folder does not exist"));
    }
    if options.replacements.is_empty() {
        return Err(bad("replacements", "name at least one text to turn into a prompt (the project's name, for a start)"));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for r in &options.replacements {
        let key_ok = r.key.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && r.key.chars().all(|c| c.is_ascii_alphanumeric());
        if !key_ok {
            return Err(bad(&r.key, "a key is letters and digits and starts with a lowercase letter, like projectName"));
        }
        if r.text.chars().count() < 2 {
            return Err(bad(&r.key, "the text to replace is too short: it would match inside other words"));
        }
        if !seen.insert(&r.key) {
            return Err(bad(&r.key, "this key is used twice"));
        }
    }

    let mut skipped = Vec::new();
    let found = read_project(project, options, &mut skipped)?;
    if found.is_empty() {
        return Err(bad(&project.display().to_string(), "the project has no files left once what is left out is removed"));
    }

    // text that already looks like a token cannot be told apart from one, and the engine has no way to write a literal
    let lookalikes: Vec<&str> = found
        .iter()
        .filter(|f| std::str::from_utf8(&f.bytes).is_ok_and(has_token_shape) || has_token_shape(&f.path))
        .map(|f| f.path.as_str())
        .collect();
    if !lookalikes.is_empty() {
        return Err(bad(
            lookalikes[0],
            format!(
                "{} file(s) already contain text shaped like a token ({{__name__}}) and the engine cannot write one literally: {}",
                lookalikes.len(),
                lookalikes.iter().take(5).copied().collect::<Vec<_>>().join(", ")
            ),
        ));
    }

    // every form of every text, longest first, so that `my-app-api` wins over `my-app`
    let mut forms: Vec<(String, String, usize)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for (index, r) in options.replacements.iter().enumerate() {
        for (form, token) in Forms::of(r).0 {
            // two texts can have a form in common ("hello-world" and "Hello World" are both HelloWorld in Pascal case):
            // the first one named keeps it, and the template renders the same either way
            if let Some((_, _, first)) = forms.iter().find(|(f, _, _)| *f == form) {
                notes.push(format!(
                    "'{form}' is a form of both '{}' and '{}': it was written as a token of '{}', the one named first",
                    options.replacements[*first].key, r.key, options.replacements[*first].key
                ));
                continue;
            }
            forms.push((form, token, index));
        }
    }
    forms.sort_by_key(|f| std::cmp::Reverse(f.0.len()));

    let mut occurrences = vec![0usize; options.replacements.len()];
    let mut files_changed: Vec<BTreeSet<String>> = vec![BTreeSet::new(); options.replacements.len()];
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut renames: BTreeMap<String, String> = BTreeMap::new();

    for f in &found {
        // the path, segment by segment (so a `.gitignore` is recognised by its own name)
        let mut path_counts = vec![0usize; options.replacements.len()];
        let segments: Vec<String> = f
            .path
            .split('/')
            .map(|seg| {
                let renamed = GIT_FILES.iter().find(|(real, _)| *real == seg).map(|(real, stored)| {
                    // the manifest says what the stored name is written as: `_gitignore` becomes `.gitignore`
                    renames.insert(stored.to_string(), real.to_string());
                    stored.to_string()
                });
                renamed.unwrap_or_else(|| replace_all(seg, &forms, &mut path_counts))
            })
            .collect();
        let new_path = format!("template/{}", segments.join("/"));

        let mut content_counts = vec![0usize; options.replacements.len()];
        let bytes = match std::str::from_utf8(&f.bytes) {
            Ok(text) => replace_all(text, &forms, &mut content_counts).into_bytes(),
            Err(_) => f.bytes.clone(), // an image or another binary file: copied as it is
        };
        for i in 0..options.replacements.len() {
            let n = path_counts[i] + content_counts[i];
            occurrences[i] += n;
            if n > 0 {
                files_changed[i].insert(f.path.clone());
            }
        }
        if out.insert(new_path.clone(), bytes).is_some() {
            return Err(bad(&f.path, format!("two files would be written to '{new_path}'")));
        }
    }

    let counts: Vec<ReplacementCount> = options
        .replacements
        .iter()
        .enumerate()
        .map(|(i, r)| ReplacementCount { key: r.key.clone(), text: r.text.clone(), occurrences: occurrences[i], files: files_changed[i].len() })
        .collect();
    if let Some(none) = counts.iter().find(|c| c.occurrences == 0) {
        return Err(bad(&none.key, format!("the text '{}' does not appear anywhere in the project", none.text)));
    }

    // ---- the manifest
    let first = &options.replacements[0];
    let folder_ok = validate_package_name(&first.text).is_empty() && !first.text.contains('/');
    let rule = if folder_ok {
        Some("package-name")
    } else if validate_solution_name(&first.text).is_empty() {
        Some("solution-name")
    } else {
        None
    };
    let mut prompts: Vec<Value> = options
        .replacements
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut p = json!({
                "key": r.key, "type": "text",
                "label": r.label.clone().unwrap_or_else(|| if i == 0 { "Project name".to_string() } else { r.key.clone() }),
                "default": r.text,
            });
            if i == 0 {
                if let Some(rule) = rule {
                    p["rule"] = json!(rule);
                }
            }
            p
        })
        .collect();

    let root_package: Option<Value> = out.get("template/package.json").and_then(|b| serde_json::from_slice(b).ok());
    let mut manifest = json!({
        "id": options.id,
        "version": options.version,
        "prompts": [],
        "folderFrom": first.key,
        "include": [ { "folder": "template" } ],
    });
    if !renames.is_empty() {
        manifest["include"][0]["rename"] = json!(renames);
    }
    if root_package.is_some() {
        prompts.push(json!({ "key": "install", "type": "bool", "label": "Run install afterwards", "default": false }));
        manifest["post"] = json!([ { "action": "install", "when": { "key": "install", "equals": true } } ]);
    }
    manifest["prompts"] = json!(prompts);
    if let Some(node) = root_package.as_ref().and_then(|p| p.pointer("/engines/node")).and_then(Value::as_str) {
        manifest["requires"] = json!({ "node": node });
    }
    let mut manifest_json = serde_json::to_string_pretty(&manifest).expect("a json! value always serializes");
    manifest_json.push('\n');

    let mut files = out;
    files.insert("manifest.json".to_string(), manifest_json.clone().into_bytes());
    let draft = TemplateDraft { manifest_json, files, skipped, counts, notes };

    verify(&draft, options, &found)?;
    Ok(draft)
}

/// Scaffolds the draft with the original texts as the answers and compares the result with the project.
fn verify(draft: &TemplateDraft, options: &CreateOptions, found: &[Found]) -> Result<()> {
    // the manifest has to be one the engine accepts, and the template has to render in every branch
    let source = draft.source();
    let report = check_with_manifest(&draft.manifest_json, &source, &CheckOptions { limits: options.limits, ..CheckOptions::default() });
    if !report.ok() {
        let first = &report.problems[0];
        return Err(bad("template", format!("the draft does not check clean ({}: {})", first.at, first.message)));
    }

    let manifest = Manifest::from_json(&draft.manifest_json)?;
    let mut answers = Answers::new();
    for r in &options.replacements {
        answers.insert(r.key.clone(), Value::String(r.text.clone()));
    }
    let plan = plan_dry(
        &manifest,
        &source,
        &answers,
        PlanOptions {
            location: PathBuf::from("forge-create"),
            package_manager: None,
            guids: Box::new(SequentialGuids::new()),
            secrets: Box::new(SequentialSecrets::new()),
            check_long_paths: false,
            limits: options.limits,
        },
    )?;

    let mut differences: Vec<String> = Vec::new();
    let expected: BTreeMap<&str, &Vec<u8>> = found.iter().map(|f| (f.path.as_str(), &f.bytes)).collect();
    for path in plan.files.paths() {
        match expected.get(path) {
            None => differences.push(format!("{path}: made, but not in the project")),
            Some(original) => {
                if plan.files.get(path) != Some(original.as_slice()) {
                    differences.push(format!("{path}: differs from the project"));
                }
            }
        }
    }
    for path in expected.keys() {
        if !plan.files.contains(path) {
            differences.push(format!("{path}: in the project, but the template does not make it"));
        }
    }
    if differences.is_empty() {
        Ok(())
    } else {
        Err(bad(
            "template",
            format!(
                "the draft does not reproduce the project ({} difference(s)): {}",
                differences.len(),
                differences.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::DirSource;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-create-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A small project called my-app.
    fn project() -> Tmp {
        let t = Tmp::new();
        t.write("package.json", "{\n  \"name\": \"my-app\",\n  \"version\": \"0.1.0\",\n  \"engines\": { \"node\": \">=20.0.0\" }\n}\n");
        t.write("README.md", "# my-app\n\nMyApp is made by Acme. See MY_APP and myApp.\n");
        t.write("src/MyApp.ts", "export class MyApp { name = 'my-app'; owner = 'Acme'; }\n");
        t.write("src/my-app.css", "/* my-app */\n");
        t.write(".gitignore", "node_modules\ndist\n");
        t.write("public/logo.png", "\u{1}\u{2}binary");
        // left out
        t.write("node_modules/x/index.js", "module.exports = 1\n");
        t.write("dist/out.js", "built\n");
        t.write(".env", "SECRET=1\n");
        t.write("package-lock.json", "{}\n");
        t
    }

    fn options() -> CreateOptions {
        CreateOptions::new("my-template", "1.0.0", vec![Replacement::new("projectName", "my-app"), Replacement::new("company", "Acme")])
    }

    #[test]
    fn a_project_becomes_a_template_that_scaffolds_back_to_the_project() {
        let t = project();
        let draft = create_template(&t.0, &options()).unwrap();

        let source = draft.source();
        let read = |p: &str| String::from_utf8(source.read_file(p).unwrap()).unwrap();
        // names and contents became tokens, in every form
        assert!(read("template/package.json").contains("\"name\": \"{__projectName__}\""));
        assert!(read("template/README.md").contains("# {__projectName__}"));
        assert!(read("template/README.md").contains("{__projectNamePascal__} is made by {__company__}"));
        assert!(read("template/README.md").contains("{__projectNameAllCaps__} and {__projectNameCamel__}"));
        assert!(read("template/src/{__projectNamePascal__}.ts").contains("class {__projectNamePascal__}"));
        assert!(source.read_file("template/src/{__projectName__}.css").is_ok());
        // a git file is stored under another name, and the manifest renames it back
        assert!(source.read_file("template/_gitignore").is_ok() && source.read_file("template/.gitignore").is_err());
        assert!(draft.manifest_json.contains("\"_gitignore\": \".gitignore\""));
        // a binary file is copied as it is
        assert_eq!(source.read_file("template/public/logo.png").unwrap(), "\u{1}\u{2}binary".as_bytes());

        // what was left out is listed with a reason
        let skipped: BTreeMap<&str, &str> = draft.skipped.iter().map(|s| (s.path.as_str(), s.reason.as_str())).collect();
        for path in ["node_modules", "dist", ".env", "package-lock.json"] {
            assert!(skipped.contains_key(path), "{path} not listed: {skipped:?}");
        }
        assert!(skipped[".env"].contains("secret") && skipped["package-lock.json"].contains("lock file"));
        assert_eq!(draft.file_count(), 6);

        // the manifest: a prompt per text, the name as the folder, an install step because there is a package.json, the Node range
        let m = Manifest::from_json(&draft.manifest_json).unwrap();
        assert_eq!(m.folder_from.as_deref(), Some("projectName"));
        assert_eq!(m.prompts.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(), ["projectName", "company", "install"]);
        assert_eq!(m.requires.as_ref().and_then(|r| r.node.as_deref()), Some(">=20.0.0"));
        assert!(draft.manifest_json.contains("\"rule\": \"package-name\""));

        // counts: the name is in package.json, README.md, MyApp.ts (name and content) and my-app.css (name and content)
        let name = &draft.counts[0];
        assert_eq!((name.key.as_str(), name.files), ("projectName", 4));
        assert!(name.occurrences >= 8, "{name:?}");
        assert_eq!(draft.counts[1].files, 2, "{:?}", draft.counts[1]); // README, MyApp.ts
    }

    #[test]
    fn the_draft_scaffolds_a_different_project_with_the_names_changed_everywhere() {
        let t = project();
        let draft = create_template(&t.0, &options()).unwrap();
        let m = Manifest::from_json(&draft.manifest_json).unwrap();
        let answers: Answers = serde_json::from_value(json!({ "projectName": "cool-shop", "company": "Globex" })).unwrap();
        let plan = plan_dry(&m, &draft.source(), &answers, PlanOptions::new("x")).unwrap();
        let get = |p: &str| String::from_utf8(plan.files.get(p).unwrap().to_vec()).unwrap();
        assert!(get("package.json").contains("\"name\": \"cool-shop\""));
        assert_eq!(get("README.md"), "# cool-shop\n\nCoolShop is made by Globex. See COOL_SHOP and coolShop.\n");
        assert!(plan.files.contains("src/CoolShop.ts") && plan.files.contains("src/cool-shop.css"));
        assert!(plan.files.contains(".gitignore") && !plan.files.contains("_gitignore"));
    }

    #[test]
    fn a_draft_can_be_written_checked_and_used_from_the_folder() {
        let t = project();
        let draft = create_template(&t.0, &options()).unwrap();
        let out = Tmp::new();
        let target = out.0.join("my-template");
        let outcome = draft.write_to(&target).unwrap();
        assert_eq!(outcome.files_written, 7, "six files and the manifest");
        // it is an ordinary generator folder now
        let report = crate::check::check_template(&DirSource::new(&target), &CheckOptions::default());
        assert!(report.ok(), "{report}");
        // and a folder that exists is not written over
        assert!(draft.write_to(&target).is_err());
    }

    #[test]
    fn what_cannot_be_made_into_a_template_is_refused_with_the_reason() {
        let t = project();
        let err = |o: CreateOptions| create_template(&t.0, &o).unwrap_err().to_string();

        assert!(err(CreateOptions::new("x", "1.0.0", vec![])).contains("at least one text"));
        assert!(err(CreateOptions::new("x", "1.0.0", vec![Replacement::new("Project", "my-app")])).contains("starts with a lowercase letter"));
        assert!(err(CreateOptions::new("x", "1.0.0", vec![Replacement::new("a", "x")])).contains("too short"));
        assert!(err(CreateOptions::new("x", "1.0.0", vec![Replacement::new("a", "my-app"), Replacement::new("a", "Acme")])).contains("used twice"));
        assert!(err(CreateOptions::new("x", "1.0.0", vec![Replacement::new("a", "nowhere-in-it")])).contains("does not appear"));
        assert!(err(CreateOptions::new("x", "nope", vec![Replacement::new("a", "my-app")])).contains("version"));
        assert!(create_template(&t.0.join("missing"), &options()).is_err());
    }

    #[test]
    fn text_shaped_like_a_token_is_refused_because_it_cannot_be_written_literally() {
        let t = project();
        t.write("docs/notes.md", "Use {__name__} in your own templates.\n");
        let err = create_template(&t.0, &options()).unwrap_err().to_string();
        assert!(err.contains("shaped like a token") && err.contains("docs/notes.md"), "{err}");
    }

    #[test]
    fn the_longest_text_wins_and_a_replacement_is_never_searched_again() {
        let t = Tmp::new();
        t.write("a.txt", "my-app-api uses my-app\n");
        t.write("package.json", "{ \"name\": \"my-app\" }\n");
        let o = CreateOptions::new("x", "1.0.0", vec![Replacement::new("projectName", "my-app"), Replacement::new("api", "my-app-api")]);
        let draft = create_template(&t.0, &o).unwrap();
        let text = String::from_utf8(draft.source().read_file("template/a.txt").unwrap()).unwrap();
        assert_eq!(text, "{__api__} uses {__projectName__}\n");
    }

    #[test]
    fn two_texts_with_a_form_in_common_are_told_apart_by_order_and_a_note_says_so() {
        let t = Tmp::new();
        t.write("package.json", "{ \"name\": \"hello-world\" }\n");
        t.write("src/HelloWorld.ts", "// Hello World, hello-world, HelloWorld\n");
        let o = CreateOptions::new("x", "1.0.0", vec![Replacement::new("projectName", "hello-world"), Replacement::new("partName", "Hello World")]);
        let draft = create_template(&t.0, &o).unwrap();
        let text = String::from_utf8(draft.source().read_file("template/src/{__projectNamePascal__}.ts").unwrap()).unwrap();
        // the literal "Hello World" (with a space) is only partName's; the forms they share belong to the first
        assert_eq!(text, "// {__partName__}, {__projectName__}, {__projectNamePascal__}\n");
        assert!(draft.notes.iter().any(|n| n.contains("'HelloWorld'") && n.contains("'projectName'")), "{:?}", draft.notes);
        // and nothing is lost: it scaffolds back to the project (create_template checked that)
    }

    #[test]
    fn a_link_in_the_project_is_left_out_and_not_followed() {
        let t = project();
        let outside = Tmp::new();
        outside.write("secret.txt", "do not read\n");
        let made = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&outside.0, t.0.join("linked")).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(&outside.0, t.0.join("linked")).is_ok()
                    || std::process::Command::new("cmd")
                        .args(["/C", "mklink", "/J"])
                        .arg(t.0.join("linked"))
                        .arg(&outside.0)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
            }
        };
        if !made {
            eprintln!("cannot make a link here, skipping");
            return;
        }
        let draft = create_template(&t.0, &options()).unwrap();
        assert!(draft.skipped.iter().any(|s| s.path == "linked" && s.reason.contains("a link")), "{:?}", draft.skipped);
        assert!(draft.source().list_files("").unwrap().iter().all(|p| !p.contains("secret")));
    }

    #[test]
    fn the_project_is_held_to_the_limits() {
        let t = project();
        let mut o = options();
        o.limits = Limits { max_files: 3, ..Limits::default() };
        assert!(matches!(create_template(&t.0, &o), Err(ScaffoldError::TooLarge(_))));
        o.limits = Limits { max_file_bytes: 10, ..Limits::default() };
        assert!(matches!(create_template(&t.0, &o), Err(ScaffoldError::TooLarge(_))));
    }

    #[test]
    fn excluded_names_are_left_out_and_a_project_without_package_json_has_no_install_step() {
        let t = Tmp::new();
        t.write("main.py", "# my-app\nprint('my-app')\n");
        t.write("scratch/notes.txt", "my-app notes\n");
        let mut o = CreateOptions::new("py", "1.0.0", vec![Replacement::new("projectName", "my-app")]);
        o.exclude.push("scratch".into());
        let draft = create_template(&t.0, &o).unwrap();
        assert!(draft.skipped.iter().any(|s| s.path == "scratch"));
        let m = Manifest::from_json(&draft.manifest_json).unwrap();
        assert_eq!(m.prompts.len(), 1);
        assert!(m.post.is_empty());
        assert!(m.requires.is_none());
    }
}
