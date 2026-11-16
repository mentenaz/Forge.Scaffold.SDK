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

fn major_minor(version: &str) -> Option<(u64, u64)> {
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
}
