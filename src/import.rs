//! Importing a package into an existing SPFx solution: a folder with a manifest, a `package.json` that lists what it needs,
//! and some source. The engine copies the source, merges the needs into the solution's `package.json`, and says what it found
//! as data. It runs no command, makes no connection, and never deletes or overwrites.
//!
//! * [`plan_import`] makes the plan, [`apply_import`] writes it, and the undo record is the one of any add
//!   ([`ImportPlan::undo_record`]).
//! * [`inspect_import`] tells, before anything is planned, what the solution already has of the package: the state of each
//!   module (a form's toggles) and the outcome of each dependency.
//!
//! What it does, in the order the plan does it:
//!
//! 1. The files: the package's `core` and the modules the host chose are copied below the target, keeping their folders.
//!    A file that is not there is created; one that is there and the same (line endings do not count) is skipped; one that
//!    differs is **kept as it is** and listed. A file whose name looks like a secret (`.env`, `*.pem`, ...) is not copied.
//! 2. The needs ([`DependencyOutcome`]): each of the package's `dependencies` and `peerDependencies` is compared with what the
//!    solution has. What the solution supplies itself (the SPFx packages, `react`, `typescript`, `@types/*`) is never
//!    changed, and it must be there. A dependency the solution lacks is added to its `package.json`; one it has is left as it
//!    is, and a version that does not fit is reported, never changed. Ranges follow npm; when the engine cannot tell whether
//!    two fit, the answer is a clash saying so, never a guess.
//! 3. A package the code imports that neither the package nor the solution declares is a warning.
//!
//! Not built yet: upgrading a package the solution has, families of packages that move together, projects that are not SPFx,
//! the authoring checks of a package, and a scan of the contents of the files for secrets.

use crate::add::{apply_add, existing_like, goes_through_a_link, package_json_warnings_of, AddFile, AddOptions, AddOutcome, AddPlan};
use crate::create::is_secret_name;
use crate::detect::SpfxSolution;
use crate::error::{Result, ScaffoldError};
use crate::manifest::{ImportModule, ImportSpec, Manifest, MergeHelper, PackageKind, PackageManager, PostAction};
use crate::merge::{merge_file, read as read_json};
use crate::node_range::{allows, exact_version, lowest};
use crate::plan::PostStepDescriptor;
use crate::source::TemplateSource;
use crate::stage::{normalize_rel, StagedFs, WriteOutcome, WriteProgress};
use crate::validate::ValidationIssue;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------------------------------------- the types

/// Where the version of an installed package was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HaveFrom {
    /// `node_modules/<name>/package.json`: what is installed.
    NodeModules,
    /// `package-lock.json` (formats 2 and 3): what the lock says will be installed.
    Lock,
    /// The lowest version the solution's own range allows. The least reliable: the minimum of `^4.2.0` is 4.2.0, whatever
    /// is installed.
    Declared,
}

/// What an import does with one need of the package. Each name is a package name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DependencyOutcome {
    /// The solution does not have it: it is added to the solution's `dependencies`.
    Add {
        /// The package.
        name: String,
        /// The range the package asks for.
        range: String,
    },
    /// The solution has it, at a version the package's range allows. Nothing changes.
    Skip {
        /// The package.
        name: String,
        /// The version the solution has.
        have: String,
        /// The range the package asks for.
        want: String,
        /// Where `have` was read from.
        have_from: HaveFrom,
    },
    /// The solution has an older version than the range allows, in the same major version. Reported; an upgrade is not
    /// applied (it is not built yet), so the solution keeps what it has.
    UpgradeAvailable {
        /// The package.
        name: String,
        /// The version the solution has.
        from: String,
        /// The range the package asks for.
        to: String,
    },
    /// The solution has a version that does not fit (another major version, or no version fits both), or the engine could
    /// not tell. Never changed.
    Clash {
        /// The package.
        name: String,
        /// The version the solution has (empty when the range itself is the problem).
        have: String,
        /// The range the package asks for.
        want: String,
        /// Why, in a sentence.
        reason: String,
    },
    /// The solution supplies this itself (an SPFx package, `react`, `typescript`, `@types/*`, or a name the package lists as
    /// provided) and has it. Never changed.
    Untouched {
        /// The package.
        name: String,
        /// `spfx`, `react`, `typescript`, `types` or `provided`.
        category: String,
    },
    /// A package the solution must supply is not there. **Stops the import.**
    MissingRequired {
        /// The package.
        name: String,
        /// The range the package asks for.
        range: String,
        /// `spfx`, `react`, `typescript`, `types` or `provided`.
        category: String,
    },
    /// A `peerDependency` of the package the solution lacks. **Stops the import.**
    MissingPeer {
        /// The package.
        name: String,
        /// The range the package asks for.
        range: String,
    },
}

impl DependencyOutcome {
    /// The package this is about.
    pub fn name(&self) -> &str {
        match self {
            Self::Add { name, .. }
            | Self::Skip { name, .. }
            | Self::UpgradeAvailable { name, .. }
            | Self::Clash { name, .. }
            | Self::Untouched { name, .. }
            | Self::MissingRequired { name, .. }
            | Self::MissingPeer { name, .. } => name,
        }
    }

    /// Whether this stops the import: the solution lacks something the package cannot work without.
    pub fn blocks(&self) -> bool {
        matches!(self, Self::MissingRequired { .. } | Self::MissingPeer { .. })
    }
}

/// What the solution has of a module (or of the shared core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleStatus {
    /// None of its files is there.
    Absent,
    /// Every file is there and the same as the package's (line endings do not count).
    Present,
    /// Every file is there, and at least one differs: it was changed after the import.
    Modified,
    /// Some files are there and some are missing.
    Partial,
}

/// The state of one module, or of the shared core: paths only, never the content of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleState {
    /// The name the host chooses it by (`core` for the shared files).
    pub name: String,
    /// What a form shows: the module's title, or its name.
    pub title: String,
    /// The summary.
    pub state: ModuleStatus,
    /// Files that are there and the same, below the solution.
    pub present: Vec<String>,
    /// Files that are not there.
    pub missing: Vec<String>,
    /// Files that are there and differ.
    pub differs: Vec<String>,
}

/// What [`inspect_import`] tells: how much of the package the solution already has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportState {
    /// The shared files, which are always copied: not a toggle.
    pub core: ModuleState,
    /// One entry per module of the package, in the manifest's order. A form's toggles.
    pub modules: Vec<ModuleState>,
    /// The outcome of each need of the package, as an import would find it now.
    pub dependencies: Vec<DependencyOutcome>,
}

/// What [`plan_import`] needs besides the solution, the package and its manifest.
#[non_exhaustive]
pub struct ImportOptions {
    /// The modules to import, by name (the shared core is always imported). A name the package does not list is an error.
    pub modules: Vec<String>,
    /// The folder name below the target. Default: made from the manifest's id (`@scope/name` becomes `scope-name`).
    pub slug: Option<String>,
    /// The options of any add: the limits, and the package manager for an `install` step.
    pub add: AddOptions,
}

impl ImportOptions {
    /// No module chosen, the default folder name and the default add options.
    pub fn new() -> Self {
        Self { modules: Vec::new(), slug: None, add: AddOptions::new() }
    }
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// What an import would do, as data. Writes nothing.
#[derive(Debug, Clone)]
pub struct ImportPlan {
    /// The files and the `package.json` merge, and the warnings: write it with [`apply_import`].
    pub add: AddPlan,
    /// The outcome of each need of the package.
    pub dependencies: Vec<DependencyOutcome>,
    /// Text for the user after the import: the package's own notes, and what was not added.
    pub notes: Vec<String>,
}

impl ImportPlan {
    /// Whether some need of the package clashes with what the solution has (it is left as it is: the files are still
    /// imported, and the package may not work until the clash is settled by hand).
    pub fn has_clashes(&self) -> bool {
        self.dependencies.iter().any(|d| matches!(d, DependencyOutcome::Clash { .. }))
    }

    /// Describes this import as an [`UndoRecord`](crate::UndoRecord). **Call it before [`apply_import`]**, as for any add.
    pub fn undo_record(&self, label: &str) -> crate::undo::UndoRecord {
        self.add.undo_record(label)
    }
}

/// Writes an import plan: the same checks and the same rollback as [`apply_add`](crate::apply_add).
pub fn apply_import(plan: &ImportPlan, on_progress: impl FnMut(WriteProgress)) -> Result<WriteOutcome> {
    apply_add(&plan.add, on_progress)
}

// ------------------------------------------------------------------------------------------------- reading the package

fn bad(msg: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Incompatible(msg.into())
}

fn invalid(field: &str, message: &str) -> ScaffoldError {
    ScaffoldError::Validation(vec![ValidationIssue::new(field, message)])
}

fn spec_of(manifest: &Manifest) -> Result<&ImportSpec> {
    manifest
        .import
        .as_ref()
        .ok_or_else(|| bad("this generator is not a package: its manifest has no 'import' section"))
}

fn make_slug(given: Option<&str>, id: &str) -> Result<String> {
    let flat = given.unwrap_or(id).trim().trim_start_matches('@').replace('/', "-").to_lowercase();
    let ok = !flat.is_empty()
        && !flat.starts_with('.')
        && !flat.contains("..")
        && flat.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(flat)
    } else {
        Err(invalid("slug", "the folder name is lowercase letters, digits, '.', '_' and '-' (a scoped name becomes scope-name)"))
    }
}

fn target_of(spec: &ImportSpec, slug: &str) -> Result<Option<String>> {
    if let Some(t) = &spec.target {
        return Ok(Some(normalize_rel(t)?));
    }
    Ok(match spec.kind {
        PackageKind::Service => Some(format!("src/services/{slug}")),
        PackageKind::Ui => Some(format!("src/components/{slug}")),
        PackageKind::Library => None,
        #[allow(unreachable_patterns)]
        _ => Some(format!("src/services/{slug}")),
    })
}

/// The files of one path of the package (a file, or a folder and everything in it): `(path below root, bytes)`.
fn read_path(source: &dyn TemplateSource, root: &str, path: &str, skipped: &mut Vec<String>) -> Result<Vec<(String, Vec<u8>)>> {
    let rel = normalize_rel(path)?;
    let full = format!("{}/{rel}", root.trim_end_matches('/'));
    let secret = |name: &str| name.rsplit('/').next().is_some_and(is_secret_name);
    if let Ok(bytes) = source.read_file(&full) {
        if secret(&rel) {
            skipped.push(rel);
            return Ok(Vec::new());
        }
        return Ok(vec![(rel, bytes)]);
    }
    let files = source
        .list_files(&full)
        .map_err(|_| ScaffoldError::Template { path: full.clone(), reason: "the package has no such file or folder".into() })?;
    let mut out = Vec::new();
    for f in files {
        let inner = format!("{rel}/{f}");
        if secret(&inner) {
            skipped.push(inner);
            continue;
        }
        out.push((inner, source.read_file(&format!("{full}/{f}"))?));
    }
    Ok(out)
}

/// The files that would be written below the target, by destination path, core first. Two paths that name the same file
/// are one.
type Files = Vec<(String, Vec<u8>)>;

struct Collected {
    core: Files,
    modules: Vec<(ImportModule, Files)>,
    skipped: Vec<String>,
}

fn collect(source: &dyn TemplateSource, spec: &ImportSpec, target: &str, which: &[&ImportModule]) -> Result<Collected> {
    let mut skipped = Vec::new();
    let place = |files: Vec<(String, Vec<u8>)>| -> Vec<(String, Vec<u8>)> {
        let mut seen: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (rel, bytes) in files {
            seen.entry(format!("{target}/{rel}")).or_insert(bytes);
        }
        seen.into_iter().collect()
    };
    let mut core = Vec::new();
    for p in &spec.core {
        core.extend(read_path(source, &spec.root, p, &mut skipped)?);
    }
    let mut modules = Vec::new();
    for m in which {
        let mut files = Vec::new();
        for p in &m.paths {
            files.extend(read_path(source, &spec.root, p, &mut skipped)?);
        }
        modules.push(((*m).clone(), place(files)));
    }
    Ok(Collected { core: place(core), modules, skipped })
}

/// The same bytes, or the same text with other line endings.
fn same(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let plain = |x: &[u8]| -> Vec<u8> { x.iter().enumerate().filter(|(i, c)| !(**c == b'\r' && x.get(i + 1) == Some(&b'\n'))).map(|(_, c)| *c).collect() };
    plain(a) == plain(b)
}

fn read_package_json(source: &dyn TemplateSource) -> Result<Value> {
    let text = source
        .read_text("package.json")
        .map_err(|_| ScaffoldError::Template { path: "package.json".into(), reason: "the package has no package.json that lists what it needs".into() })?;
    read_json(text.trim_start_matches('\u{feff}'))
        .map(|(v, _)| v)
        .map_err(|e| ScaffoldError::Template { path: "package.json".into(), reason: e })
}

fn read_solution_json(root: &Path) -> Result<(Value, Vec<u8>)> {
    let path = root.join("package.json");
    let bytes = fs::read(&path).map_err(|e| ScaffoldError::Io { path: path.clone(), source: e })?;
    let (value, _) = read_json(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}'))
        .map_err(|e| ScaffoldError::Merge { file: "package.json".into(), reason: e })?;
    Ok((value, bytes))
}

// ---------------------------------------------------------------------------------------------------- the dependencies

/// What the solution supplies itself, and the category to say it by.
fn provided_category(name: &str, extra: &[String]) -> Option<&'static str> {
    if name.starts_with("@microsoft/sp-") || name == "@microsoft/decorators" {
        Some("spfx")
    } else if name == "react" || name == "react-dom" {
        Some("react")
    } else if name == "typescript" {
        Some("typescript")
    } else if name.starts_with("@types/") {
        Some("types")
    } else if extra.iter().any(|p| p == name) {
        Some("provided")
    } else {
        None
    }
}

fn solution_has(pkg: &Value, name: &str) -> Option<String> {
    ["dependencies", "devDependencies"]
        .iter()
        .find_map(|s| pkg.get(*s).and_then(|d| d.get(name)).and_then(Value::as_str))
        .map(str::to_string)
}

/// A package the solution does not declare but has installed (a package another one brought): its version. Used for what the
/// solution supplies and for peers, which need to be resolvable, not necessarily declared.
fn installed_only(root: &Path, name: &str) -> Option<String> {
    fs::read(root.join("node_modules").join(name).join("package.json")).ok().as_deref().and_then(version_in)
}

fn version_in(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(bytes).ok()?.get("version")?.as_str().map(str::to_string)
}

/// The version the solution has of `name`: what is installed, else what the lock says, else the lowest its own range allows.
fn installed_version(root: &Path, name: &str, declared: &str) -> (String, HaveFrom) {
    if let Some(v) = fs::read(root.join("node_modules").join(name).join("package.json")).ok().as_deref().and_then(version_in) {
        return (v, HaveFrom::NodeModules);
    }
    if let Some(v) = fs::read(root.join("package-lock.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|lock| lock.get("packages")?.get(format!("node_modules/{name}"))?.get("version")?.as_str().map(str::to_string))
    {
        return (v, HaveFrom::Lock);
    }
    match lowest(declared) {
        Some((a, b, c)) => (format!("{a}.{b}.{c}"), HaveFrom::Declared),
        None => (declared.to_string(), HaveFrom::Declared),
    }
}

/// `1.0.0-beta.1`: a digit, a hyphen and a letter or digit.
fn has_prerelease(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(3).any(|w| w[0].is_ascii_digit() && w[1] == '-' && w[2].is_ascii_alphanumeric())
}

/// A range npm reads from the registry (not a path, a URL, a tag or a workspace).
fn is_registry_range(range: &str) -> bool {
    let r = range.trim();
    !(r.contains(':') || r.contains('/') || r.starts_with("git") || r.chars().next().is_some_and(|c| c.is_ascii_alphabetic() && !matches!(c, 'x' | 'X')))
}

fn compare(name: &str, want: &str, declared: &str, root: &Path) -> DependencyOutcome {
    let (have, from) = installed_version(root, name, declared);
    let unsure = |why: &str| DependencyOutcome::Clash {
        name: name.to_string(),
        have: have.clone(),
        want: want.to_string(),
        reason: format!("could not tell: {why}"),
    };
    if !is_registry_range(want) {
        return unsure(&format!("'{want}' is not a version range"));
    }
    if has_prerelease(want) || has_prerelease(&have) {
        return unsure("a pre-release version");
    }
    let Some(h) = exact_version(&have) else { return unsure(&format!("the solution's version '{have}' is not a version number")) };
    match allows(want, &have) {
        Err(e) => unsure(&e.to_string()),
        Ok(true) => DependencyOutcome::Skip { name: name.into(), have, want: want.into(), have_from: from },
        Ok(false) => match lowest(want) {
            Some(low) if low.0 == h.0 && h < low => DependencyOutcome::UpgradeAvailable { name: name.into(), from: have, to: want.into() },
            Some(_) => DependencyOutcome::Clash {
                name: name.into(),
                reason: format!("the solution has {have} and the package needs {want}: no version fits both"),
                have,
                want: want.into(),
            },
            None => unsure("the range cannot be read"),
        },
    }
}

fn entries<'a>(pkg: &'a Value, section: &str) -> Vec<(&'a str, &'a str)> {
    pkg.get(section)
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.as_str(), v))).collect())
        .unwrap_or_default()
}

/// Every need of the package against what the solution has: dependencies first, then peers, each in the package's order.
fn dependency_outcomes(root: &Path, solution: &Value, package: &Value, extra: &[String]) -> Vec<DependencyOutcome> {
    let mut out = Vec::new();
    for (name, range) in entries(package, "dependencies") {
        let provided = provided_category(name, extra);
        let have = solution_has(solution, name).or_else(|| provided.and_then(|_| installed_only(root, name)));
        out.push(match (provided, have) {
            (Some(category), Some(_)) => DependencyOutcome::Untouched { name: name.into(), category: category.into() },
            (Some(category), None) => DependencyOutcome::MissingRequired { name: name.into(), range: range.into(), category: category.into() },
            (None, Some(declared)) => compare(name, range, &declared, root),
            (None, None) if is_registry_range(range) => DependencyOutcome::Add { name: name.into(), range: range.into() },
            (None, None) => DependencyOutcome::Clash {
                name: name.into(),
                have: String::new(),
                want: range.into(),
                reason: format!("could not tell: '{range}' is not a version range"),
            },
        });
    }
    for (name, range) in entries(package, "peerDependencies") {
        // a peer has to be resolvable: declared, or installed as part of something else
        let have = solution_has(solution, name).or_else(|| installed_only(root, name));
        out.push(match (have, provided_category(name, extra)) {
            (None, _) => DependencyOutcome::MissingPeer { name: name.into(), range: range.into() },
            (Some(_), Some(category)) => DependencyOutcome::Untouched { name: name.into(), category: category.into() },
            (Some(declared), None) => compare(name, range, &declared, root),
        });
    }
    out
}

// ----------------------------------------------------------------------------------------- the imports the code makes

const NODE_BUILTINS: &[&str] = &[
    "assert", "buffer", "child_process", "crypto", "events", "fs", "http", "https", "os", "path", "process", "querystring", "stream",
    "string_decoder", "timers", "tty", "url", "util", "vm", "zlib",
];

/// The names a file imports: `from "x"`, `import "x"`, `import("x")` and `require("x")`.
fn specifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        for needle in ["from ", "import ", "import(", "require("] {
            let mut rest = t;
            while let Some(i) = rest.find(needle) {
                let after = rest[i + needle.len()..].trim_start();
                if let Some(q) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) {
                    if let Some(end) = after[1..].find(q) {
                        out.push(after[1..1 + end].to_string());
                    }
                }
                rest = &rest[i + needle.len()..];
            }
        }
    }
    out
}

/// The package a specifier is about: `@pnp/sp/webs` is `@pnp/sp`, `lodash/fp` is `lodash`; `None` for a path, a built-in
/// module or a name the solution's `tsconfig` maps to a folder.
fn package_of(spec: &str, aliases: &[String]) -> Option<String> {
    if spec.is_empty() || spec.starts_with('.') || spec.starts_with('/') || spec.starts_with("node:") {
        return None;
    }
    if aliases.iter().any(|a| spec == a || spec.starts_with(&format!("{a}/"))) {
        return None;
    }
    let mut parts = spec.split('/');
    let first = parts.next()?;
    let name = if first.starts_with('@') { format!("{first}/{}", parts.next()?) } else { first.to_string() };
    (!NODE_BUILTINS.contains(&name.as_str())).then_some(name)
}

/// The names the solution's `tsconfig.json` maps through `compilerOptions.paths` (`@Services/*` gives `@Services`).
fn tsconfig_aliases(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("tsconfig.json")) else { return Vec::new() };
    let Ok((doc, _)) = read_json(text.trim_start_matches('\u{feff}')) else { return Vec::new() };
    doc.pointer("/compilerOptions/paths")
        .and_then(Value::as_object)
        .map(|m| m.keys().map(|k| k.trim_end_matches('*').trim_end_matches('/').to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default()
}

/// One warning for each package the copied code imports that neither the package nor the solution declares.
fn undeclared_imports(files: &[(String, Vec<u8>)], package: &Value, solution: &Value, root: &Path) -> Vec<String> {
    let mut declared: HashSet<String> = HashSet::new();
    for section in ["dependencies", "peerDependencies", "devDependencies", "optionalDependencies"] {
        for v in [package, solution] {
            if let Some(m) = v.get(section).and_then(Value::as_object) {
                declared.extend(m.keys().cloned());
            }
        }
    }
    let aliases = tsconfig_aliases(root);
    let mut first_seen: BTreeMap<String, String> = BTreeMap::new();
    for (path, bytes) in files {
        let ext = path.rsplit('.').next().unwrap_or("");
        if !matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mts" | "cts") {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else { continue };
        for s in specifiers(text) {
            if let Some(name) = package_of(&s, &aliases) {
                if !declared.contains(&name) && !declared.contains(&format!("@types/{name}")) {
                    first_seen.entry(name).or_insert_with(|| path.clone());
                }
            }
        }
    }
    first_seen
        .into_iter()
        .map(|(name, file)| format!("{file} imports '{name}', which neither the package nor the solution declares in its package.json: the build will not find it"))
        .collect()
}

// -------------------------------------------------------------------------------------------------------- the entry points

/// The modules the host chose, from the package's list; an unknown name is an error.
fn chosen<'a>(spec: &'a ImportSpec, names: &[String]) -> Result<Vec<&'a ImportModule>> {
    let mut out: Vec<&ImportModule> = Vec::new();
    for n in names {
        match spec.modules.iter().find(|m| m.name.eq_ignore_ascii_case(n)) {
            Some(m) if !out.iter().any(|o| o.name == m.name) => out.push(m),
            Some(_) => {}
            None => {
                let known: Vec<&str> = spec.modules.iter().map(|m| m.name.as_str()).collect();
                return Err(invalid("modules", &format!("the package has no module '{n}' (it has: {})", known.join(", "))));
            }
        }
    }
    Ok(out)
}

/// Plans importing the package in `source` into `solution`.
///
/// The files go below the package's target (`src/services/<slug>/` for a service) and keep their folders; a file that is
/// already there and the same is skipped, one that differs is kept and listed in [`AddPlan::kept`]. The package's needs are
/// compared with the solution's own and merged into its `package.json` (see [`DependencyOutcome`]). A solution that lacks
/// something the package cannot work without (an SPFx package, a peer) is an error and nothing is planned.
///
/// Nothing is written: that is [`apply_import`]. An import can be repeated with more modules: the shared files that are
/// already there are skipped.
///
/// # Example
///
/// ```
/// use forge_scaffold::{detect_spfx, plan_import, DirSource, ImportOptions, Manifest};
/// use std::fs;
///
/// # let scratch = std::env::temp_dir().join(format!("forge-plan-import-doc-{}", std::process::id()));
/// # let _ = fs::remove_dir_all(&scratch);
/// # let solution = scratch.join("solution");
/// # let package = scratch.join("package");
/// # fs::create_dir_all(solution.join("config"))?;
/// # fs::create_dir_all(package.join("src"))?;
/// # fs::write(solution.join("package.json"), r#"{ "name": "my-sol", "dependencies": { "@microsoft/sp-core-library": "1.21.1" } }"#)?;
/// # fs::write(solution.join("config/config.json"), "{}")?;
/// # fs::write(solution.join("config/package-solution.json"), "{}")?;
/// # fs::write(package.join("manifest.json"), r#"{ "id": "acme-data", "version": "1.0.0", "prompts": [], "import": { "core": ["ctx.ts"] } }"#)?;
/// # fs::write(package.join("package.json"), r#"{ "dependencies": { "date-fns": "^3.6.0" } }"#)?;
/// # fs::write(package.join("src/ctx.ts"), "export const ctx = 1;\n")?;
/// let manifest = Manifest::from_json(&fs::read_to_string(package.join("manifest.json"))?)?;
/// let solution = detect_spfx(&solution)?.expect("an SPFx solution");
///
/// let plan = plan_import(&solution, &DirSource::new(&package), &manifest, ImportOptions::new())?;
/// // the file lands below the target, and the package's need is added to package.json
/// assert!(plan.add.files.iter().any(|f| f.path == "src/services/acme-data/ctx.ts"));
/// assert!(plan.add.files.iter().any(|f| f.path == "package.json"));
/// assert_eq!(plan.dependencies[0].name(), "date-fns");
/// # fs::remove_dir_all(&scratch)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn plan_import(solution: &SpfxSolution, source: &dyn TemplateSource, manifest: &Manifest, options: ImportOptions) -> Result<ImportPlan> {
    let spec = spec_of(manifest)?;
    let slug = make_slug(options.slug.as_deref(), &manifest.id)?;
    let target = target_of(spec, &slug)?;
    let which = chosen(spec, &options.modules)?;
    let root = &solution.root;

    let package = read_package_json(source)?;
    let (solution_pkg, solution_bytes) = read_solution_json(root)?;

    // what the package needs
    let dependencies = dependency_outcomes(root, &solution_pkg, &package, &spec.provided);
    let blocking: Vec<String> = dependencies
        .iter()
        .filter_map(|d| match d {
            DependencyOutcome::MissingPeer { name, range } => Some(format!("{name} {range} (a peer dependency of the package)")),
            DependencyOutcome::MissingRequired { name, range, category } => Some(format!("{name} {range} (the solution supplies {category} packages itself)")),
            _ => None,
        })
        .collect();
    if !blocking.is_empty() {
        return Err(bad(format!(
            "the package needs what the solution does not have, so nothing was planned: {}",
            blocking.join("; ")
        )));
    }

    // the files
    let mut files: Vec<AddFile> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut staged: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(target) = &target {
        let mut listings = HashMap::new();
        if let Some(existing) = existing_like(root, target, &mut listings) {
            if existing != *target {
                return Err(bad(format!("'{target}' would differ from '{existing}', which is already in the solution, only by letter case, so nothing was planned")));
            }
        }
        let collected = collect(source, spec, target, &which)?;
        for s in &collected.skipped {
            notes.push(format!("{s} was not copied: its name says it may hold a secret"));
        }
        let mut stage = StagedFs::with_limits(options.add.limits);
        let mut all: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (path, bytes) in collected.core.into_iter().chain(collected.modules.into_iter().flat_map(|(_, f)| f)) {
            all.entry(path).or_insert(bytes);
        }
        for (path, bytes) in &all {
            stage.insert(path, bytes.clone())?; // the limits, and a path that differs only by letter case
        }
        for (path, bytes) in all {
            let on_disk = root.join(&path);
            match fs::symlink_metadata(&on_disk) {
                Err(_) => files.push(AddFile { path: path.clone(), outcome: AddOutcome::New, content: bytes.clone(), previous: None }),
                Ok(m) if m.is_dir() => return Err(ScaffoldError::ExistsInSolution(path)),
                Ok(_) => {
                    let have = fs::read(&on_disk).map_err(|e| ScaffoldError::Io { path: on_disk.clone(), source: e })?;
                    if !same(&have, &bytes) {
                        kept.push(path.clone());
                    }
                }
            }
            staged.push((path, bytes));
        }
    }

    // package.json: the needs the solution lacks, added after the existing ones, formatted as the file is
    let adds: Map<String, Value> = dependencies
        .iter()
        .filter_map(|d| match d {
            DependencyOutcome::Add { name, range } => Some((name.clone(), Value::String(range.clone()))),
            _ => None,
        })
        .collect();
    if !adds.is_empty() {
        let incoming = serde_json::to_vec(&json!({ "dependencies": adds })).expect("a json! value serializes");
        let merged = merge_file(MergeHelper::PackageJson, "package.json", &solution_bytes, &incoming)?;
        if merged != solution_bytes {
            files.push(AddFile { path: "package.json".into(), outcome: AddOutcome::Merged, content: merged, previous: Some(solution_bytes) });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(f) = files.iter().find(|f| goes_through_a_link(root, &f.path)) {
        return Err(ScaffoldError::LinkInSolution(f.path.clone()));
    }

    // what to tell the user
    let mut warnings = solution.warnings.clone();
    if !kept.is_empty() {
        let shown: Vec<&str> = kept.iter().take(5).map(String::as_str).collect();
        warnings.push(format!(
            "{} existing file(s) differ from the package and were left as they are: {}{}",
            kept.len(),
            shown.join(", "),
            if kept.len() > shown.len() { ", ..." } else { "" }
        ));
    }
    for d in &dependencies {
        match d {
            DependencyOutcome::Clash { name, have, want, reason } => {
                warnings.push(format!("{name}: the package needs {want}, the solution has {}: {reason}. It was left as it is", if have.is_empty() { "nothing usable" } else { have }))
            }
            DependencyOutcome::UpgradeAvailable { name, from, to } => {
                warnings.push(format!("{name}: the package needs {to} and the solution has {from}. It was left as it is (upgrading is not built yet)"))
            }
            _ => {}
        }
    }
    warnings.extend(package_json_warnings_of(&files));
    warnings.extend(undeclared_imports(&staged, &package, &solution_pkg, root));
    if !which.is_empty() || !spec.modules.is_empty() {
        let missing: Vec<&str> = spec.modules.iter().filter(|m| !which.iter().any(|w| w.name == m.name)).map(|m| m.name.as_str()).collect();
        if !missing.is_empty() && which.is_empty() {
            notes.push(format!("no module was chosen, so only the shared files are imported (the package has: {})", missing.join(", ")));
        }
    }
    let dev: Vec<&str> = entries(&package, "devDependencies").into_iter().map(|(n, _)| n).collect();
    if !dev.is_empty() {
        notes.push(format!("the package's devDependencies ({}) are the author's own tools and were not added", dev.join(", ")));
    }
    notes.extend(spec.notes.iter().cloned());

    let changes_package_json = files.iter().any(|f| f.path == "package.json");
    let post = changes_package_json
        .then(|| PostStepDescriptor {
            action: PostAction::Install,
            cwd: root.clone(),
            package_manager: Some(options.add.package_manager.unwrap_or(PackageManager::Npm)),
        })
        .into_iter()
        .collect();
    Ok(ImportPlan { add: AddPlan { root: root.clone(), files, kept, warnings, post }, dependencies, notes })
}

fn state_of(name: &str, title: &str, root: &Path, files: &[(String, Vec<u8>)]) -> ModuleState {
    let (mut present, mut missing, mut differs) = (Vec::new(), Vec::new(), Vec::new());
    for (path, bytes) in files {
        match fs::read(root.join(path)) {
            Err(_) => missing.push(path.clone()),
            Ok(have) if same(&have, bytes) => present.push(path.clone()),
            Ok(_) => differs.push(path.clone()),
        }
    }
    let state = if present.is_empty() && differs.is_empty() {
        ModuleStatus::Absent
    } else if !missing.is_empty() {
        ModuleStatus::Partial
    } else if differs.is_empty() {
        ModuleStatus::Present
    } else {
        ModuleStatus::Modified
    };
    ModuleState { name: name.into(), title: title.into(), state, present, missing, differs }
}

/// What the solution already has of the package: the state of its shared files and of each module (a form shows each module
/// as a switch that is on when the module is [`Present`](ModuleStatus::Present) or [`Modified`](ModuleStatus::Modified)),
/// and the outcome of each of its needs. Reads the solution and the package and writes nothing; a missing peer is data here
/// ([`DependencyOutcome::MissingPeer`]), not an error.
///
/// The engine never deletes, so a module that is on cannot be turned off by an import: removing it is done by hand, or by
/// undoing the import while its files are untouched.
pub fn inspect_import(solution: &SpfxSolution, source: &dyn TemplateSource, manifest: &Manifest) -> Result<ImportState> {
    let spec = spec_of(manifest)?;
    let slug = make_slug(None, &manifest.id)?;
    let target = target_of(spec, &slug)?;
    let root = &solution.root;
    let package = read_package_json(source)?;
    let (solution_pkg, _) = read_solution_json(root)?;
    let dependencies = dependency_outcomes(root, &solution_pkg, &package, &spec.provided);

    let all: Vec<&ImportModule> = spec.modules.iter().collect();
    let (core, modules) = match &target {
        Some(target) => {
            let c = collect(source, spec, target, &all)?;
            (
                state_of("core", "Shared files", root, &c.core),
                c.modules.iter().map(|(m, files)| state_of(&m.name, m.title.as_deref().unwrap_or(&m.name), root, files)).collect(),
            )
        }
        None => (state_of("core", "Shared files", root, &[]), Vec::new()),
    };
    Ok(ImportState { core, modules, dependencies })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_of_an_import_is_found_from_every_way_to_write_one() {
        let none: Vec<String> = vec![];
        assert_eq!(package_of("@pnp/sp/webs", &none).as_deref(), Some("@pnp/sp"));
        assert_eq!(package_of("lodash/fp", &none).as_deref(), Some("lodash"));
        assert_eq!(package_of("date-fns", &none).as_deref(), Some("date-fns"));
        assert_eq!(package_of("./x", &none), None);
        assert_eq!(package_of("../x", &none), None);
        assert_eq!(package_of("node:fs", &none), None);
        assert_eq!(package_of("fs", &none), None);
        let aliases = vec!["@Services".to_string(), "@Types".to_string()];
        assert_eq!(package_of("@Services/Get", &aliases), None);
        assert_eq!(package_of("@Services", &aliases), None);
        assert_eq!(package_of("@Servicesx/y", &aliases).as_deref(), Some("@Servicesx/y"));
    }

    #[test]
    fn specifiers_are_read_from_imports_exports_dynamic_imports_and_requires() {
        let text = "import a from 'one';\nimport type { B } from \"two\";\nexport * from 'three/sub';\nimport 'four';\nconst c = await import('five');\nconst d = require(\"six\");\n// import x from 'commented'\n * from 'doc'\nconst n = obj.from + 1;\n";
        assert_eq!(specifiers(text), ["one", "two", "three/sub", "four", "five", "six"]);
    }

    #[test]
    fn a_prerelease_and_a_non_registry_range_are_told_apart() {
        assert!(has_prerelease("1.0.0-beta.1") && has_prerelease("^4.0.0-rc.0"));
        assert!(!has_prerelease("^4.2.0") && !has_prerelease(">=1.2.3 <2"));
        assert!(is_registry_range("^4.2.0") && is_registry_range(">=1 <3 || ^5") && is_registry_range("*") && is_registry_range("x") && is_registry_range("~1.2"));
        assert!(!is_registry_range("file:../x") && !is_registry_range("git+https://x") && !is_registry_range("latest") && !is_registry_range("user/repo") && !is_registry_range("workspace:*"));
    }

    #[test]
    fn the_same_text_with_other_line_endings_is_the_same_file() {
        assert!(same(b"a\r\nb\r\n", b"a\nb\n"));
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"a\nb", b"a\nc"));
        assert!(!same(b"a\rb", b"a\nb"), "a lone carriage return is content");
    }

    #[test]
    fn a_slug_is_made_safe() {
        assert_eq!(make_slug(None, "acme-data-services").unwrap(), "acme-data-services");
        assert_eq!(make_slug(None, "@Acme/Data").unwrap(), "acme-data");
        assert_eq!(make_slug(Some("My.Services"), "x").unwrap(), "my.services");
        for bad in ["", "..", "a/../b", ".hidden", "has space", "a:b", "a\\b"] {
            assert!(make_slug(Some(bad), "x").is_err(), "{bad:?}");
        }
    }

    fn outcome_of(want: &str, solution_range: &str, tmp: &Path) -> DependencyOutcome {
        compare("zod", want, solution_range, tmp)
    }

    #[test]
    fn a_version_is_compared_with_a_range_the_way_npm_does() {
        let tmp = std::env::temp_dir().join(format!("forge-import-compare-{}", std::process::id()));
        // no node_modules and no lock here: the version is the lowest the solution's own range allows
        let o = |want: &str, have: &str| outcome_of(want, have, &tmp);
        assert!(matches!(o("^3.23.0", "^3.23.4"), DependencyOutcome::Skip { have_from: HaveFrom::Declared, ref have, .. } if have == "3.23.4"));
        assert!(matches!(o("^3.20.0", "^3.23.4"), DependencyOutcome::Skip { .. }), "a newer minor of the same major fits");
        assert!(matches!(o("^3.24.0", "^3.23.4"), DependencyOutcome::UpgradeAvailable { .. }), "older, same major");
        assert!(matches!(o("^4.0.0", "^3.23.4"), DependencyOutcome::Clash { .. }), "another major");
        assert!(matches!(o("^2.0.0", "^3.23.4"), DependencyOutcome::Clash { .. }), "the solution is newer: no upgrade helps");
        assert!(matches!(o("~3.23.0", "^3.24.0"), DependencyOutcome::Clash { .. }), "same major, newer than the range allows");
        assert!(matches!(o(">=3 <5", "^3.23.4"), DependencyOutcome::Skip { .. }));
        assert!(matches!(o("*", "^3.23.4"), DependencyOutcome::Skip { .. }));
        for unsure in [("^3.0.0-rc.1", "^3.23.4"), ("^3.23.0", "latest"), ("file:../z", "^3.23.4"), ("^3.23.0", "workspace:*"), ("1.2 - 1.4", "^1.3.0")] {
            match o(unsure.0, unsure.1) {
                DependencyOutcome::Clash { reason, .. } => assert!(reason.starts_with("could not tell"), "{unsure:?}: {reason}"),
                other => panic!("{unsure:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_installed_version_wins_over_the_lock_and_the_lock_over_the_range() {
        let root = std::env::temp_dir().join(format!("forge-import-installed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("node_modules/zod")).unwrap();
        fs::write(root.join("package-lock.json"), r#"{ "lockfileVersion": 3, "packages": { "node_modules/zod": { "version": "3.21.0" }, "node_modules/@s/p": { "version": "2.5.0" } } }"#).unwrap();
        // the lock alone
        assert_eq!(installed_version(&root, "zod", "^3.0.0"), ("3.21.0".to_string(), HaveFrom::Lock));
        assert_eq!(installed_version(&root, "@s/p", "^2.0.0"), ("2.5.0".to_string(), HaveFrom::Lock));
        // node_modules as well: it is what is really installed
        fs::write(root.join("node_modules/zod/package.json"), r#"{ "name": "zod", "version": "3.22.4" }"#).unwrap();
        assert_eq!(installed_version(&root, "zod", "^3.0.0"), ("3.22.4".to_string(), HaveFrom::NodeModules));
        // neither: the range's own minimum
        assert_eq!(installed_version(&root, "other", "^1.4.2"), ("1.4.2".to_string(), HaveFrom::Declared));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn every_kind_of_need_gets_its_own_outcome() {
        let root = std::env::temp_dir().join(format!("forge-import-kinds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("node_modules/@microsoft/sp-http")).unwrap();
        fs::write(root.join("node_modules/@microsoft/sp-http/package.json"), r#"{ "version": "1.21.1" }"#).unwrap();
        let solution = json!({
            "dependencies": { "@microsoft/sp-core-library": "1.21.1", "react": "17.0.1", "zod": "^3.23.0" },
            "devDependencies": { "typescript": "~5.3.3", "tool": "^1.0.0" }
        });
        let package = json!({
            "dependencies": { "@microsoft/sp-core-library": "^1.21.0", "react-dom": "^17.0.0", "zod": "^3.20.0", "tool": "^1.2.0", "fresh": "^2.0.0", "weird": "file:../w" },
            "peerDependencies": { "@microsoft/sp-http": "^1.21.0", "@pnp/sp": "^4.0.0", "react": "^17.0.0" },
            "devDependencies": { "build-tool": "^9.0.0" }
        });
        let o = dependency_outcomes(&root, &solution, &package, &[]);
        let by = |n: &str, nth: usize| o.iter().filter(|d| d.name() == n).nth(nth).unwrap_or_else(|| panic!("{n}: {o:?}")).clone();
        assert!(matches!(by("@microsoft/sp-core-library", 0), DependencyOutcome::Untouched { ref category, .. } if category == "spfx"));
        assert!(matches!(by("react-dom", 0), DependencyOutcome::MissingRequired { ref category, .. } if category == "react"), "the solution has no react-dom");
        assert!(matches!(by("zod", 0), DependencyOutcome::Skip { .. }));
        assert!(matches!(by("tool", 0), DependencyOutcome::UpgradeAvailable { .. }), "a dev dependency of the solution counts as the solution's");
        assert_eq!(by("fresh", 0), DependencyOutcome::Add { name: "fresh".into(), range: "^2.0.0".into() });
        assert!(matches!(by("weird", 0), DependencyOutcome::Clash { ref reason, .. } if reason.starts_with("could not tell")));
        assert!(matches!(by("@microsoft/sp-http", 0), DependencyOutcome::Untouched { .. }), "installed, though not declared");
        assert!(matches!(by("@pnp/sp", 0), DependencyOutcome::MissingPeer { .. }));
        assert!(matches!(by("react", 0), DependencyOutcome::Untouched { ref category, .. } if category == "react"));
        assert!(o.iter().all(|d| d.name() != "build-tool"), "the author's dev dependencies are not read");
        assert_eq!(o.iter().filter(|d| d.blocks()).count(), 2, "react-dom and @pnp/sp stop the import");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn what_the_solution_supplies_itself_is_told_by_name() {
        let extra = vec!["rxjs".to_string()];
        assert_eq!(provided_category("@microsoft/sp-http", &extra), Some("spfx"));
        assert_eq!(provided_category("react-dom", &extra), Some("react"));
        assert_eq!(provided_category("typescript", &extra), Some("typescript"));
        assert_eq!(provided_category("@types/react", &extra), Some("types"));
        assert_eq!(provided_category("rxjs", &extra), Some("provided"));
        assert_eq!(provided_category("@pnp/sp", &extra), None);
        assert_eq!(provided_category("@microsoft/microsoft-graph-client", &extra), None);
    }
}
