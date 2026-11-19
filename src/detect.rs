//! Finding an existing solution on disk.
//!
//! The "add to an existing solution" flow starts here. [`detect_solution`] walks
//! up from a folder the user picked until it finds one that satisfies a
//! [`DetectRule`]: a marker file (for SPFx `.yo-rc.json`) that holds the
//! generator's own state, plus a few files the solution must have. The result
//! says what the solution is (version, framework, build system), so the caller
//! can pick the matching pack.
//!
//! Detection only reads. Nothing here writes, and nothing runs a command.

use crate::error::{Result, ScaffoldError};
use crate::stage::normalize_rel;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// What makes a folder an existing solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectRule {
    /// File that holds the generator state, relative to the solution root.
    pub marker: String,
    /// Key in the marker file whose object is the state. A marker without this
    /// key belongs to another tool and does not count.
    pub state_key: String,
    /// Files that must also exist, relative to the solution root.
    pub requires: Vec<String>,
}

impl DetectRule {
    /// SPFx solutions made by `@microsoft/generator-sharepoint`.
    ///
    /// No dependency is required in `package.json`: library solutions have no
    /// `@microsoft/sp-*` dependency, and they are still solutions.
    pub fn spfx() -> Self {
        Self {
            marker: ".yo-rc.json".into(),
            state_key: "@microsoft/generator-sharepoint".into(),
            requires: vec!["package.json".into(), "config/config.json".into()],
        }
    }
}

/// How the solution is built. SPFx records this per solution, not per version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSystem {
    Gulp,
    Heft,
    /// The marker does not say and the version does not settle it.
    Unknown,
}

/// An existing solution found by [`detect_solution`].
#[derive(Debug, Clone)]
pub struct Solution {
    /// The folder that holds the marker file.
    pub root: PathBuf,
    /// The generator's state object from the marker file.
    pub state: Map<String, Value>,
}

impl Solution {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.state.get(key)?.as_str()
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.state.get(key)?.as_bool()
    }

    /// The SPFx version the solution was created with (`"1.23.2"`).
    pub fn version(&self) -> Option<&str> {
        self.text("version")
    }

    /// The template the solution was created with (`react`, `none`, `minimal`).
    pub fn framework(&self) -> Option<&str> {
        self.text("template")
    }

    pub fn solution_name(&self) -> Option<&str> {
        self.text("solutionName")
    }

    /// `useGulp` when the marker has it. The gulp-only versions (1.20, 1.21) do
    /// not write it, so a missing field on a version below 1.22 means gulp.
    pub fn build_system(&self) -> BuildSystem {
        match self.flag("useGulp") {
            Some(true) => BuildSystem::Gulp,
            Some(false) => BuildSystem::Heft,
            None => match self.version().and_then(major_minor) {
                Some((1, minor)) if minor < 22 => BuildSystem::Gulp,
                _ => BuildSystem::Unknown,
            },
        }
    }

    /// Reads a file inside the solution as text.
    pub fn read_text(&self, rel: &str) -> Result<String> {
        let rel = normalize_rel(rel)?;
        let path = self.root.join(&rel);
        let text = fs::read_to_string(&path).map_err(|e| ScaffoldError::Template {
            path: rel.clone(),
            reason: e.to_string(),
        })?;
        Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
    }

    /// Reads a JSON file inside the solution.
    pub fn read_json(&self, rel: &str) -> Result<Value> {
        let text = self.read_text(rel)?;
        serde_json::from_str(&text).map_err(|e| ScaffoldError::Template {
            path: rel.to_string(),
            reason: format!("not valid JSON: {e}"),
        })
    }

    /// The version range `package.json` gives a package, from `dependencies`
    /// first, then `devDependencies`. `None` if it is in neither.
    pub fn dependency_version(&self, name: &str) -> Result<Option<String>> {
        let pkg = self.read_json("package.json")?;
        for section in ["dependencies", "devDependencies"] {
            if let Some(v) = pkg.get(section).and_then(|s| s.get(name)).and_then(Value::as_str) {
                return Ok(Some(v.to_string()));
            }
        }
        Ok(None)
    }
}

/// Walks up from `start` (a folder, or a file inside the solution) and returns
/// the nearest folder that satisfies `rule`, or `None` if there is none.
///
/// A broken marker file in a folder that otherwise looks like a solution is an
/// error, not a silent miss: it usually means a hand edit the user wants to hear
/// about.
pub fn detect_solution(start: &Path, rule: &DetectRule) -> Result<Option<Solution>> {
    let dir = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    if !dir.is_dir() {
        return Err(ScaffoldError::InvalidPath {
            path: start.display().to_string(),
            reason: "the folder does not exist".into(),
        });
    }
    for candidate in dir.ancestors() {
        if let Some(found) = check_dir(candidate, rule)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn check_dir(dir: &Path, rule: &DetectRule) -> Result<Option<Solution>> {
    let marker = dir.join(&rule.marker);
    if !marker.is_file() || !rule.requires.iter().all(|r| dir.join(r).is_file()) {
        return Ok(None);
    }
    let fail = |reason: String| ScaffoldError::Template {
        path: marker.display().to_string(),
        reason,
    };
    let text = fs::read_to_string(&marker).map_err(|e| fail(e.to_string()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let doc: Value = serde_json::from_str(text).map_err(|e| fail(format!("not valid JSON: {e}")))?;
    // Another tool's marker: keep walking.
    let Some(state) = doc.get(&rule.state_key).and_then(Value::as_object) else {
        return Ok(None);
    };
    Ok(Some(Solution {
        root: dir.to_path_buf(),
        state: state.clone(),
    }))
}

// ------------------------------------------------------------ from the files

/// An SPFx solution recognised from its own files, however it was made
/// (`yo`, the official CLI, by hand).
///
/// The files are the truth. `.yo-rc.json` is read when it is there, but only as
/// a hint: the new CLI does not write it, and it can be stale after hand edits.
#[derive(Debug, Clone)]
pub struct SpfxSolution {
    /// The folder with `package.json` and `config/`.
    pub root: PathBuf,
    /// The SPFx version, from the `@microsoft/sp-*` / `spfx-*` dependencies
    /// (`"1.20.0"`); the marker's `version` only when they give none.
    pub version: Option<String>,
    pub build_system: BuildSystem,
    /// `"react"` when `react` is a dependency; otherwise the marker's `template`.
    pub framework: Option<String>,
    /// `name` in `package.json`.
    pub package_name: Option<String>,
    /// `engines.node` in `package.json`: the Node versions the solution builds with.
    pub node_range: Option<String>,
    /// The `.yo-rc.json` state, if there is a usable one. A hint only.
    pub marker: Option<Map<String, Value>>,
    /// Things worth telling the user (SPFx packages on different versions, an
    /// unreadable `.yo-rc.json`).
    pub warnings: Vec<String>,
}

impl SpfxSolution {
    /// Can this solution be installed and built on the Node that is installed
    /// (what `node -v` prints)? The host asks before it runs `npm install`.
    pub fn node_check(&self, installed: &str) -> Result<crate::node_range::NodeCheck> {
        crate::node_range::check_node(self.node_range.as_deref(), installed)
    }
}

/// The files that make a folder look like an SPFx solution.
const SPFX_FILES: &[&str] = &["package.json", "config/config.json", "config/package-solution.json"];
/// Packages whose version is the SPFx version, best evidence first.
const SPFX_VERSION_PACKAGES: &[&str] = &[
    "@microsoft/sp-core-library",
    "@microsoft/sp-build-web",
    "@microsoft/spfx-heft-plugins",
    "@microsoft/sp-webpart-base",
];

/// Walks up from `start` and returns the nearest SPFx solution, or `None` when
/// the folder is not inside one. This is the question the host asks first: a
/// solution means "add to it", `None` means "scaffold a new one".
///
/// A folder that has the solution files but an unreadable `package.json` is an
/// error, not a silent miss: it is almost certainly a solution with a hand edit
/// the user wants to hear about.
pub fn detect_spfx(start: &Path) -> Result<Option<SpfxSolution>> {
    let dir = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    if !dir.is_dir() {
        return Err(ScaffoldError::InvalidPath {
            path: start.display().to_string(),
            reason: "the folder does not exist".into(),
        });
    }
    for candidate in dir.ancestors() {
        if let Some(found) = check_spfx_dir(candidate)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn check_spfx_dir(dir: &Path) -> Result<Option<SpfxSolution>> {
    if !SPFX_FILES.iter().all(|f| dir.join(f).is_file()) {
        return Ok(None);
    }
    let pkg_path = dir.join("package.json");
    let fail = |reason: String| ScaffoldError::Template {
        path: pkg_path.display().to_string(),
        reason,
    };
    let text = fs::read_to_string(&pkg_path).map_err(|e| fail(e.to_string()))?;
    let pkg: Value = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text))
        .map_err(|e| fail(format!("not valid JSON: {e}")))?;

    // name -> version range, from dependencies and devDependencies
    let mut deps: Vec<(&str, &str)> = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(map) = pkg.get(section).and_then(Value::as_object) {
            deps.extend(map.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.as_str(), v))));
        }
    }
    let has = |name: &str| deps.iter().any(|(n, _)| *n == name);
    let version_of = |name: &str| deps.iter().find(|(n, _)| *n == name).map(|(_, v)| clean_version(v));

    // Not SPFx unless an SPFx package is there (libraries have no sp-* runtime
    // dependency, but they have the build tooling).
    if !deps.iter().any(|(n, _)| n.starts_with("@microsoft/sp-") || n.starts_with("@microsoft/spfx-")) {
        return Ok(None);
    }

    let mut warnings = Vec::new();

    // Version: the first evidence package that has one; warn if others disagree.
    let mut versions: Vec<(String, String)> = SPFX_VERSION_PACKAGES
        .iter()
        .filter_map(|n| version_of(n).flatten().map(|v| (n.to_string(), v)))
        .collect();
    for (n, r) in &deps {
        if n.starts_with("@microsoft/sp-") && !SPFX_VERSION_PACKAGES.contains(n) {
            if let Some(v) = clean_version(r) {
                versions.push((n.to_string(), v));
            }
        }
    }
    let version = versions.first().map(|(_, v)| v.clone());
    if let Some(first) = &version {
        // Patch differences are normal (the build tooling is often a patch ahead of
        // the runtime packages, e.g. sp-build-web 1.20.2 next to sp-core-library
        // 1.20.0). A different minor version means a half-finished upgrade.
        let mut others: Vec<&str> = versions
            .iter()
            .filter(|(_, v)| major_minor(v) != major_minor(first))
            .map(|(n, _)| n.as_str())
            .collect();
        if !others.is_empty() {
            others.sort_unstable();
            warnings.push(format!(
                "the SPFx packages are not all on {first} ({}); the solution may be mid-upgrade",
                others.join(", ")
            ));
        }
    }

    // The marker, as a hint.
    let mut marker = None;
    let marker_path = dir.join(".yo-rc.json");
    if marker_path.is_file() {
        match fs::read_to_string(&marker_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(t.strip_prefix('\u{feff}').unwrap_or(&t)).ok())
        {
            Some(doc) => marker = doc.get("@microsoft/generator-sharepoint").and_then(Value::as_object).cloned(),
            None => warnings.push("`.yo-rc.json` is not valid JSON and was ignored".into()),
        }
    }
    let hint = |key: &str| marker.as_ref().and_then(|m| m.get(key)).and_then(Value::as_str).map(str::to_string);

    let scripts = pkg.get("scripts").and_then(Value::as_object);
    let heft_in_scripts = scripts
        .is_some_and(|s| s.values().filter_map(Value::as_str).any(|c| c.split_whitespace().any(|w| w == "heft")));
    let gulp = dir.join("gulpfile.js").is_file() || has("@microsoft/sp-build-web");
    let heft = dir.join("config/rig.json").is_file()
        || has("@microsoft/spfx-web-build-rig")
        || has("@microsoft/spfx-heft-plugins")
        || heft_in_scripts;
    let build_system = match (gulp, heft) {
        (true, false) => BuildSystem::Gulp,
        (false, true) => BuildSystem::Heft,
        (true, true) => {
            warnings.push("the solution has both gulp and heft files; the build system is unclear".into());
            BuildSystem::Unknown
        }
        (false, false) => match marker.as_ref().and_then(|m| m.get("useGulp")).and_then(Value::as_bool) {
            Some(true) => BuildSystem::Gulp,
            Some(false) => BuildSystem::Heft,
            None => BuildSystem::Unknown,
        },
    };

    Ok(Some(SpfxSolution {
        root: dir.to_path_buf(),
        version: version.or_else(|| hint("version")),
        build_system,
        framework: if has("react") { Some("react".to_string()) } else { hint("template") },
        package_name: pkg.get("name").and_then(Value::as_str).map(str::to_string),
        node_range: pkg
            .pointer("/engines/node")
            .and_then(Value::as_str)
            .map(str::to_string),
        marker,
        warnings,
    }))
}

/// `"1.20.0"` from a dependency range like `"~1.20.0"` or `"^1.20.0"`. `None`
/// when it is not a plain version (a tag, a URL, a range with a space).
pub(crate) fn clean_version(range: &str) -> Option<String> {
    let v = range.trim().trim_start_matches(['^', '~', '=', 'v']);
    let core = v.split(['-', '+']).next()?;
    let parts: Vec<&str> = core.split('.').collect();
    (parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
        .then(|| v.to_string())
}

pub(crate) fn major_minor(version: &str) -> Option<(u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "forge-scaffold-detect-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The marker as `yo @microsoft/sharepoint` 1.23.2 writes it.
    const YO_RC_1232: &str = r#"{
  "@microsoft/generator-sharepoint": {
    "whichFolder": "subdir",
    "solutionName": "sample-solution",
    "environment": "spo",
    "packageManager": "npm",
    "useGulp": false,
    "componentType": "webpart",
    "template": "react",
    "componentName": "AlphaWebPart",
    "version": "1.23.2",
    "libraryId": "fb0a6172-c183-458a-9a59-401a7d34dbf9"
  }
}"#;

    /// A minimal but complete SPFx solution.
    fn solution(t: &Tmp, at: &str, marker: &str) {
        let at = if at.is_empty() { String::new() } else { format!("{at}/") };
        t.write(&format!("{at}.yo-rc.json"), marker);
        t.write(&format!("{at}package.json"), "{}");
        t.write(&format!("{at}config/config.json"), "{}");
    }

    fn found(t: &Tmp, from: &str) -> Option<Solution> {
        detect_solution(&t.path(from), &DetectRule::spfx()).unwrap()
    }

    #[test]
    fn finds_the_solution_from_its_root_a_subfolder_and_a_file() {
        let t = Tmp::new("walk");
        solution(&t, "", YO_RC_1232);
        t.write("src/webparts/alphaWebPart/AlphaWebPartWebPart.ts", "x");

        let from_root = found(&t, "").unwrap();
        assert_eq!(from_root.root, t.0);
        let from_sub = found(&t, "src/webparts/alphaWebPart").unwrap();
        assert_eq!(from_sub.root, t.0);
        let from_file = found(&t, "src/webparts/alphaWebPart/AlphaWebPartWebPart.ts").unwrap();
        assert_eq!(from_file.root, t.0);
    }

    #[test]
    fn reads_what_the_marker_says() {
        let t = Tmp::new("state");
        solution(&t, "", YO_RC_1232);
        let s = found(&t, "").unwrap();
        assert_eq!(s.version(), Some("1.23.2"));
        assert_eq!(s.framework(), Some("react"));
        assert_eq!(s.solution_name(), Some("sample-solution"));
        assert_eq!(s.text("libraryId"), Some("fb0a6172-c183-458a-9a59-401a7d34dbf9"));
        assert_eq!(s.flag("useGulp"), Some(false));
        assert_eq!(s.build_system(), BuildSystem::Heft);
    }

    #[test]
    fn nothing_found_is_none_not_an_error() {
        let t = Tmp::new("none");
        t.write("src/a.ts", "x");
        assert!(found(&t, "src").is_none());
    }

    #[test]
    fn another_tools_marker_does_not_count() {
        let t = Tmp::new("other");
        solution(&t, "", r#"{ "generator-something-else": { "version": "1" } }"#);
        assert!(found(&t, "").is_none());
    }

    #[test]
    fn a_missing_required_file_is_not_a_solution() {
        let t = Tmp::new("incomplete");
        t.write(".yo-rc.json", YO_RC_1232);
        t.write("package.json", "{}");
        assert!(found(&t, "").is_none(), "config/config.json is missing");
    }

    #[test]
    fn the_nearest_solution_wins() {
        let t = Tmp::new("nested");
        solution(&t, "", YO_RC_1232);
        solution(
            &t,
            "inner",
            &YO_RC_1232.replace("sample-solution", "inner-solution"),
        );
        t.write("inner/src/a.ts", "x");
        let s = found(&t, "inner/src").unwrap();
        assert_eq!(s.solution_name(), Some("inner-solution"));
        assert_eq!(s.root, t.path("inner"));
        // From the outer folder the outer one is found.
        assert_eq!(found(&t, "").unwrap().solution_name(), Some("sample-solution"));
    }

    #[test]
    fn a_broken_marker_in_a_real_solution_is_an_error() {
        let t = Tmp::new("broken");
        solution(&t, "", "{ not json");
        let err = detect_solution(&t.0, &DetectRule::spfx()).unwrap_err();
        assert!(matches!(err, ScaffoldError::Template { .. }), "got {err}");
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let t = Tmp::new("bom");
        solution(&t, "", &format!("\u{feff}{YO_RC_1232}"));
        assert_eq!(found(&t, "").unwrap().version(), Some("1.23.2"));
    }

    #[test]
    fn a_missing_start_folder_is_an_error() {
        let t = Tmp::new("missing");
        let err = detect_solution(&t.path("nope"), &DetectRule::spfx()).unwrap_err();
        assert!(matches!(err, ScaffoldError::InvalidPath { .. }), "got {err}");
    }

    fn with_state(state: Value) -> Solution {
        Solution {
            root: PathBuf::new(),
            state: state.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn build_system_comes_from_use_gulp_or_else_from_the_version() {
        assert_eq!(with_state(json!({ "useGulp": true, "version": "1.23.2" })).build_system(), BuildSystem::Gulp);
        assert_eq!(with_state(json!({ "useGulp": false, "version": "1.22.2" })).build_system(), BuildSystem::Heft);
        // 1.20 and 1.21 are gulp-only and never write the field (verified against real yo output).
        assert_eq!(with_state(json!({ "version": "1.20.0" })).build_system(), BuildSystem::Gulp);
        assert_eq!(with_state(json!({ "version": "1.21.1" })).build_system(), BuildSystem::Gulp);
        // From 1.22 the field is always written, so its absence settles nothing.
        assert_eq!(with_state(json!({ "version": "1.22.2" })).build_system(), BuildSystem::Unknown);
        assert_eq!(with_state(json!({})).build_system(), BuildSystem::Unknown);
        assert_eq!(with_state(json!({ "version": "garbage" })).build_system(), BuildSystem::Unknown);
    }

    #[test]
    fn reads_json_and_dependency_versions_from_the_solution() {
        let t = Tmp::new("deps");
        solution(&t, "", YO_RC_1232);
        t.write(
            "package.json",
            r#"{ "dependencies": { "@microsoft/sp-core-library": "1.23.2" },
                 "devDependencies": { "typescript": "~5.8.0" } }"#,
        );
        let s = found(&t, "").unwrap();
        assert_eq!(s.dependency_version("@microsoft/sp-core-library").unwrap().as_deref(), Some("1.23.2"));
        assert_eq!(s.dependency_version("typescript").unwrap().as_deref(), Some("~5.8.0"));
        assert_eq!(s.dependency_version("left-pad").unwrap(), None);
        assert!(s.read_json("config/config.json").unwrap().is_object());
    }

    #[test]
    fn reading_cannot_leave_the_solution() {
        let t = Tmp::new("escape");
        solution(&t, "", YO_RC_1232);
        let s = found(&t, "").unwrap();
        assert!(s.read_text("../outside.txt").is_err());
        assert!(s.read_text("/etc/passwd").is_err());
        assert!(s.read_json("config/missing.json").is_err());
    }

    // --------------------------------------------------- from the files

    fn gulp_solution(t: &Tmp, at: &str) {
        let at = if at.is_empty() { String::new() } else { format!("{at}/") };
        t.write(&format!("{at}package.json"), r#"{ "name": "my-sol",
          "dependencies": { "react": "17.0.1", "@microsoft/sp-core-library": "1.20.0", "@microsoft/sp-webpart-base": "1.20.0" },
          "devDependencies": { "@microsoft/sp-build-web": "1.20.0", "gulp": "4.0.2" } }"#);
        t.write(&format!("{at}gulpfile.js"), "");
        t.write(&format!("{at}config/config.json"), "{}");
        t.write(&format!("{at}config/package-solution.json"), "{}");
    }

    #[test]
    fn a_gulp_solution_is_recognised_without_any_marker() {
        let t = Tmp::new("spfx-gulp");
        gulp_solution(&t, "");
        let s = detect_spfx(&t.0).unwrap().expect("a solution");
        assert_eq!(s.root, t.0);
        assert_eq!(s.version.as_deref(), Some("1.20.0"));
        assert_eq!(s.build_system, BuildSystem::Gulp);
        assert_eq!(s.framework.as_deref(), Some("react"));
        assert_eq!(s.package_name.as_deref(), Some("my-sol"));
        assert!(s.marker.is_none() && s.warnings.is_empty());
    }

    #[test]
    fn a_heft_solution_made_by_the_new_cli_has_no_marker_and_is_still_found() {
        let t = Tmp::new("spfx-heft");
        t.write("package.json", r#"{ "name": "x", "scripts": { "build": "heft test --clean" },
          "dependencies": { "@microsoft/sp-core-library": "1.24.1" },
          "devDependencies": { "@microsoft/spfx-web-build-rig": "1.24.1", "@microsoft/spfx-heft-plugins": "1.24.1" } }"#);
        t.write("config/config.json", "{}");
        t.write("config/package-solution.json", "{}");
        t.write("config/rig.json", "{}");
        let s = detect_spfx(&t.0).unwrap().unwrap();
        assert_eq!((s.version.as_deref(), s.build_system), (Some("1.24.1"), BuildSystem::Heft));
        assert_eq!(s.framework, None);
    }

    #[test]
    fn it_walks_up_from_inside_the_solution_and_the_marker_is_only_a_hint() {
        let t = Tmp::new("spfx-up");
        gulp_solution(&t, "");
        t.write(".yo-rc.json", YO_RC_1232);
        t.write("src/webparts/a/A.ts", "");
        let s = detect_spfx(&t.path("src/webparts/a/A.ts")).unwrap().unwrap();
        assert_eq!(s.root, t.0);
        // the files say 1.20.0, the stale marker says 1.23.2: the files win
        assert_eq!(s.version.as_deref(), Some("1.20.0"));
        assert_eq!(s.marker.as_ref().and_then(|m| m.get("solutionName")).and_then(Value::as_str), Some("sample-solution"));
    }

    #[test]
    fn a_broken_marker_is_ignored_with_a_warning_and_the_nearest_solution_wins() {
        let t = Tmp::new("spfx-bad-marker");
        gulp_solution(&t, "inner");
        gulp_solution(&t, "");
        t.write("inner/.yo-rc.json", "{ nope");
        let s = detect_spfx(&t.path("inner")).unwrap().unwrap();
        assert_eq!(s.root, t.path("inner"));
        assert!(s.warnings.iter().any(|w| w.contains(".yo-rc.json")));
    }

    #[test]
    fn mixed_spfx_versions_are_reported() {
        let t = Tmp::new("spfx-mixed");
        gulp_solution(&t, "");
        t.write("package.json", r#"{ "dependencies": { "@microsoft/sp-core-library": "1.20.0", "@microsoft/sp-property-pane": "1.19.0" },
          "devDependencies": { "@microsoft/sp-build-web": "1.20.0" } }"#);
        let s = detect_spfx(&t.0).unwrap().unwrap();
        assert_eq!(s.version.as_deref(), Some("1.20.0"));
        assert!(s.warnings.iter().any(|w| w.contains("sp-property-pane")), "{:?}", s.warnings);
    }

    #[test]
    fn tooling_a_patch_ahead_of_the_runtime_is_normal_and_does_not_warn() {
        let t = Tmp::new("spfx-patch");
        gulp_solution(&t, "");
        t.write("package.json", r#"{ "dependencies": { "@microsoft/sp-core-library": "1.20.0" },
          "devDependencies": { "@microsoft/sp-build-web": "1.20.2", "@microsoft/sp-module-interfaces": "1.20.2" } }"#);
        let s = detect_spfx(&t.0).unwrap().unwrap();
        assert_eq!(s.version.as_deref(), Some("1.20.0"));
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }

    #[test]
    fn folders_that_are_not_solutions_are_none() {
        let t = Tmp::new("spfx-none");
        assert!(detect_spfx(&t.0).unwrap().is_none());
        // the right files but nothing from SPFx in package.json
        t.write("package.json", r#"{ "dependencies": { "express": "4.0.0" } }"#);
        t.write("config/config.json", "{}");
        t.write("config/package-solution.json", "{}");
        assert!(detect_spfx(&t.0).unwrap().is_none());
        assert!(detect_spfx(&t.path("missing")).is_err());
    }

    #[test]
    fn a_solution_with_an_unreadable_package_json_is_an_error() {
        let t = Tmp::new("spfx-broken");
        gulp_solution(&t, "");
        t.write("package.json", "{ nope");
        assert!(detect_spfx(&t.0).is_err());
    }

    #[test]
    fn the_node_a_solution_needs_comes_from_its_package_json_and_the_host_can_check_it() {
        use crate::node_range::NodeCheck;
        let t = Tmp::new("spfx-node");
        gulp_solution(&t, "");
        t.write("package.json", r#"{ "name": "x", "engines": { "node": ">=18.17.1 <19.0.0" },
          "dependencies": { "@microsoft/sp-core-library": "1.20.0" }, "devDependencies": { "@microsoft/sp-build-web": "1.20.2" } }"#);
        let s = detect_spfx(&t.0).unwrap().unwrap();
        assert_eq!(s.node_range.as_deref(), Some(">=18.17.1 <19.0.0"));
        assert_eq!(s.node_check("v18.20.8").unwrap(), NodeCheck::Fine);
        assert_eq!(
            s.node_check("v22.23.0").unwrap(),
            NodeCheck::Mismatch { required: ">=18.17.1 <19.0.0".into(), installed: "22.23.0".into() }
        );

        // a solution that does not say gives no verdict
        t.write("package.json", r#"{ "dependencies": { "@microsoft/sp-core-library": "1.20.0" } }"#);
        let none = detect_spfx(&t.0).unwrap().unwrap();
        assert_eq!(none.node_range, None);
        assert_eq!(none.node_check("v22.23.0").unwrap(), NodeCheck::NoRequirement);
    }
}
