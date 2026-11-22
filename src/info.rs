//! What a host shows about an existing SPFx solution, read from its files (nothing is written):
//! its names and ids, its type (what components it holds), the SPFx version, the build system, the Node range and the
//! components in it. [`find_spfx_solutions`] finds every solution under a workspace folder, so a workspace with one solution
//! at its root, one in a subfolder, or several (a monorepo) all work the same way.
//!
//! The files are the truth: `.yo-rc.json` is not needed. A hand-broken component manifest is reported as a warning and does
//! not hide the other components.

use crate::detect::{check_spfx_dir, BuildSystem, SpfxSolution};
use crate::error::{Result, ScaffoldError};
use crate::jsonc::Doc;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Folders that never hold a solution of their own (build output, dependencies, version control).
const SKIP_DIRS: &[&str] = &[
    "node_modules", "lib", "lib-commonjs", "lib-amd", "lib-dts", "dist", "temp", "release", "sharepoint", "target", "coverage",
];
/// How many folders [`find_spfx_solutions`] looks into before it stops, so a huge tree cannot hang a host.
const MAX_DIRS: usize = 20_000;
/// How deep below `src` component manifests are looked for.
const MAX_MANIFEST_DEPTH: usize = 8;

/// What kind of component a manifest describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ComponentKind {
    WebPart,
    Extension,
    AdaptiveCardExtension,
    Library,
    /// A `componentType` this crate does not know.
    Other,
}

impl ComponentKind {
    fn from_manifest(component_type: &str) -> ComponentKind {
        match component_type {
            "WebPart" => ComponentKind::WebPart,
            "Extension" => ComponentKind::Extension,
            "AdaptiveCardExtension" => ComponentKind::AdaptiveCardExtension,
            "Library" => ComponentKind::Library,
            _ => ComponentKind::Other,
        }
    }

    /// The name used in JSON and in `SolutionInfo::type_label`.
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::WebPart => "webpart",
            ComponentKind::Extension => "extension",
            ComponentKind::AdaptiveCardExtension => "adaptiveCardExtension",
            ComponentKind::Library => "library",
            ComponentKind::Other => "other",
        }
    }
}

/// One component of a solution, from its `*.manifest.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentInfo {
    /// The manifest's `alias` (`HelloWorldWebPart`), else its file name without `.manifest.json`.
    pub name: String,
    pub kind: ComponentKind,
    /// The manifest's `id`.
    pub id: Option<String>,
    /// For an extension: `ApplicationCustomizer`, `FieldCustomizer`, ...
    pub extension_type: Option<String>,
    /// The manifest file, relative to the solution, with `/`.
    pub manifest: String,
}

/// What a host shows about one solution.
#[derive(Debug, Clone)]
pub struct SolutionInfo {
    pub root: PathBuf,
    /// `name` in `package.json`.
    pub package_name: Option<String>,
    /// `solution.name` in `config/package-solution.json`: the name SharePoint shows.
    pub solution_name: Option<String>,
    /// `solution.id`: never change it for a solution that has been deployed.
    pub solution_id: Option<String>,
    /// `solution.version` (four numbers, `1.0.0.0`).
    pub solution_version: Option<String>,
    pub spfx_version: Option<String>,
    pub build_system: BuildSystem,
    pub framework: Option<String>,
    /// `engines.node`.
    pub node_range: Option<String>,
    pub components: Vec<ComponentInfo>,
    /// Anything worth telling the user: the solution's own warnings and manifests that could not be read.
    pub warnings: Vec<String>,
}

impl SolutionInfo {
    /// The kinds of component in the solution, once each, in a fixed order (web part, extension, Adaptive Card extension,
    /// library, other) so the label does not depend on folder names.
    pub fn kinds(&self) -> Vec<ComponentKind> {
        [
            ComponentKind::WebPart,
            ComponentKind::Extension,
            ComponentKind::AdaptiveCardExtension,
            ComponentKind::Library,
            ComponentKind::Other,
        ]
        .into_iter()
        .filter(|k| self.components.iter().any(|c| c.kind == *k))
        .collect()
    }

    /// A short description of the type for a form: `web part`, `extension`, `web part and extension`, `empty`, ...,
    /// then the framework in brackets when there is one.
    pub fn type_label(&self) -> String {
        let names: Vec<&str> = self
            .kinds()
            .iter()
            .map(|k| match k {
                ComponentKind::WebPart => "web part",
                ComponentKind::Extension => "extension",
                ComponentKind::AdaptiveCardExtension => "Adaptive Card extension",
                ComponentKind::Library => "library",
                _ => "component",
            })
            .collect();
        let base = match names.as_slice() {
            [] => "no components yet".to_string(),
            [one] => (*one).to_string(),
            many => format!("{} and {}", many[..many.len() - 1].join(", "), many[many.len() - 1]),
        };
        match &self.framework {
            Some(f) => format!("{base} ({f})"),
            None => base,
        }
    }

    /// The information as JSON, for a host that is not written in Rust.
    pub fn to_json(&self) -> Value {
        json!({
            "root": self.root.display().to_string(),
            "packageName": self.package_name,
            "solutionName": self.solution_name,
            "solutionId": self.solution_id,
            "solutionVersion": self.solution_version,
            "type": self.type_label(),
            "kinds": self.kinds().iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            "spfxVersion": self.spfx_version,
            "buildSystem": format!("{:?}", self.build_system).to_lowercase(),
            "framework": self.framework,
            "nodeRange": self.node_range,
            "components": self.components.iter().map(|c| json!({
                "name": c.name,
                "kind": c.kind.as_str(),
                "id": c.id,
                "extensionType": c.extension_type,
                "manifest": c.manifest,
            })).collect::<Vec<_>>(),
            "warnings": self.warnings,
        })
    }
}

/// Reads the information about a detected solution.
pub fn solution_info(solution: &SpfxSolution) -> Result<SolutionInfo> {
    let mut warnings = solution.warnings.clone();
    let root = &solution.root;

    let (mut solution_name, mut solution_id, mut solution_version) = (None, None, None);
    let ps = root.join("config/package-solution.json");
    match fs::read_to_string(&ps).map_err(|e| e.to_string()).and_then(|t| Doc::open(t.strip_prefix('\u{feff}').unwrap_or(&t))?.value()) {
        Ok(v) => {
            let text = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
            solution_name = text("/solution/name");
            solution_id = text("/solution/id");
            solution_version = text("/solution/version");
        }
        Err(e) => warnings.push(format!("config/package-solution.json could not be read: {e}")),
    }

    let mut components = Vec::new();
    let src = root.join("src");
    if src.is_dir() {
        collect_manifests(root, &src, 0, &mut components, &mut warnings);
    }
    components.sort_by(|a, b| a.manifest.cmp(&b.manifest));

    Ok(SolutionInfo {
        root: root.clone(),
        package_name: solution.package_name.clone(),
        solution_name,
        solution_id,
        solution_version,
        spfx_version: solution.version.clone(),
        build_system: solution.build_system,
        framework: solution.framework.clone(),
        node_range: solution.node_range.clone(),
        components,
        warnings,
    })
}

fn collect_manifests(root: &Path, dir: &Path, depth: usize, out: &mut Vec<ComponentInfo>, warnings: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            continue; // never follow a link out of the solution
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            if depth < MAX_MANIFEST_DEPTH && name != "node_modules" {
                collect_manifests(root, &path, depth + 1, out, warnings);
            }
        } else if let Some(stem) = name.strip_suffix(".manifest.json") {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let parsed = fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| Doc::open(t.strip_prefix('\u{feff}').unwrap_or(&t))?.value());
            match parsed {
                Ok(v) => {
                    let Some(kind) = v.get("componentType").and_then(Value::as_str) else {
                        continue; // some other tool's *.manifest.json
                    };
                    out.push(ComponentInfo {
                        name: v.get("alias").and_then(Value::as_str).unwrap_or(stem).to_string(),
                        kind: ComponentKind::from_manifest(kind),
                        id: v.get("id").and_then(Value::as_str).map(str::to_string),
                        extension_type: v.get("extensionType").and_then(Value::as_str).map(str::to_string),
                        manifest: rel,
                    });
                }
                Err(e) => warnings.push(format!("{rel} could not be read: {e}")),
            }
        }
    }
}

/// Every SPFx solution at or below `workspace`, however the workspace is laid out. The search goes down at most
/// `max_depth` folders (0 looks only at `workspace` itself), skips dependency and build folders, hidden folders and
/// links, and does not look inside a solution it has found. Solutions come back in path order.
///
/// A solution whose `package.json` cannot be read is reported in `problems` and does not hide the others.
pub fn find_spfx_solutions(workspace: &Path, max_depth: usize) -> Result<WorkspaceScan> {
    if !workspace.is_dir() {
        return Err(ScaffoldError::InvalidPath { path: workspace.display().to_string(), reason: "the folder does not exist".into() });
    }
    let mut scan = WorkspaceScan::default();
    let mut visited = 0usize;
    walk(workspace, 0, max_depth, &mut visited, &mut scan);
    scan.solutions.sort_by(|a, b| a.root.cmp(&b.root));
    Ok(scan)
}

/// The result of [`find_spfx_solutions`].
#[derive(Debug, Default)]
pub struct WorkspaceScan {
    pub solutions: Vec<SpfxSolution>,
    /// Folders that look like a solution but could not be read, and a note if the search was cut short.
    pub problems: Vec<String>,
}

fn walk(dir: &Path, depth: usize, max_depth: usize, visited: &mut usize, scan: &mut WorkspaceScan) {
    *visited += 1;
    if *visited > MAX_DIRS {
        if *visited == MAX_DIRS + 1 {
            scan.problems.push(format!("the search stopped after {MAX_DIRS} folders"));
        }
        return;
    }
    match check_spfx_dir(dir) {
        Ok(Some(found)) => {
            scan.solutions.push(found);
            return; // a solution inside a solution is not a thing this looks for
        }
        Ok(None) => {}
        Err(e) => {
            scan.problems.push(e.to_string());
            return;
        }
    }
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        subdirs.push(entry.path());
    }
    subdirs.sort();
    for sub in subdirs {
        walk(&sub, depth + 1, max_depth, visited, scan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::detect_spfx;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-info-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

    /// A small solution under `prefix` (no trailing slash; empty for the folder itself).
    fn solution(t: &Tmp, prefix: &str, name: &str) {
        let at = |rel: &str| if prefix.is_empty() { rel.to_string() } else { format!("{prefix}/{rel}") };
        t.write(&at("package.json"), &format!(
            r#"{{ "name": "{name}", "engines": {{ "node": ">=22.14.0 < 23.0.0" }},
               "dependencies": {{ "@microsoft/sp-core-library": "1.21.1", "react": "17.0.1" }},
               "devDependencies": {{ "@microsoft/sp-build-web": "1.21.1" }} }}"#));
        t.write(&at("gulpfile.js"), "");
        t.write(&at("config/config.json"), "{ \"bundles\": {} }");
        // JSON with comments, as the SPFx generators write it
        t.write(&at("config/package-solution.json"),
            "{\n  // the solution\n  \"solution\": {\n    \"name\": \"Sample Solution\",\n    \"id\": \"11111111-2222-3333-4444-555555555555\",\n    \"version\": \"1.0.0.0\",\n  }\n}\n");
        t.write(&at("src/webparts/hello/HelloWebPart.manifest.json"),
            "{ // a web part\n \"id\": \"aaaaaaaa-0000-0000-0000-000000000001\", \"alias\": \"HelloWebPart\", \"componentType\": \"WebPart\" }");
    }

    #[test]
    fn reads_the_names_type_and_components_of_a_solution() {
        let t = Tmp::new();
        solution(&t, "", "sample");
        let sol = detect_spfx(&t.0).unwrap().unwrap();
        let info = solution_info(&sol).unwrap();
        assert_eq!(info.package_name.as_deref(), Some("sample"));
        assert_eq!(info.solution_name.as_deref(), Some("Sample Solution"));
        assert_eq!(info.solution_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        assert_eq!(info.solution_version.as_deref(), Some("1.0.0.0"));
        assert_eq!(info.spfx_version.as_deref(), Some("1.21.1"));
        assert_eq!(info.build_system, BuildSystem::Gulp);
        assert_eq!(info.node_range.as_deref(), Some(">=22.14.0 < 23.0.0"));
        assert_eq!(info.components.len(), 1);
        assert_eq!(info.components[0].name, "HelloWebPart");
        assert_eq!(info.components[0].kind, ComponentKind::WebPart);
        assert_eq!(info.components[0].manifest, "src/webparts/hello/HelloWebPart.manifest.json");
        assert_eq!(info.type_label(), "web part (react)");
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        let j = info.to_json();
        assert_eq!(j["type"], "web part (react)");
        assert_eq!(j["components"][0]["kind"], "webpart");
        assert_eq!(j["buildSystem"], "gulp");
    }

    #[test]
    fn the_type_follows_what_the_solution_holds() {
        let t = Tmp::new();
        solution(&t, "", "sample");
        t.write("src/extensions/bar/BarApplicationCustomizer.manifest.json",
            r#"{ "id": "aaaaaaaa-0000-0000-0000-000000000002", "alias": "BarApplicationCustomizer", "componentType": "Extension", "extensionType": "ApplicationCustomizer" }"#);
        let info = solution_info(&detect_spfx(&t.0).unwrap().unwrap()).unwrap();
        assert_eq!(info.type_label(), "web part and extension (react)");
        let ext = info.components.iter().find(|c| c.kind == ComponentKind::Extension).unwrap();
        assert_eq!(ext.extension_type.as_deref(), Some("ApplicationCustomizer"));

        fs::remove_dir_all(t.0.join("src")).unwrap();
        let empty = solution_info(&detect_spfx(&t.0).unwrap().unwrap()).unwrap();
        assert_eq!(empty.type_label(), "no components yet (react)");
    }

    #[test]
    fn a_broken_manifest_is_a_warning_and_other_json_files_are_ignored() {
        let t = Tmp::new();
        solution(&t, "", "sample");
        t.write("src/webparts/broken/BrokenWebPart.manifest.json", "{ not json");
        t.write("src/tools/other.manifest.json", r#"{ "name": "somebody else's file" }"#);
        let info = solution_info(&detect_spfx(&t.0).unwrap().unwrap()).unwrap();
        assert_eq!(info.components.len(), 1, "the good one is still there");
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("BrokenWebPart.manifest.json"), "{:?}", info.warnings);
    }

    #[test]
    fn finds_a_solution_at_the_root_in_a_subfolder_and_several_in_a_monorepo() {
        // at the root
        let a = Tmp::new();
        solution(&a, "", "one");
        assert_eq!(find_spfx_solutions(&a.0, 3).unwrap().solutions.len(), 1);

        // in a subfolder, and two more in a monorepo layout
        let b = Tmp::new();
        solution(&b, "apps/web", "web");
        solution(&b, "apps/intranet", "intranet");
        solution(&b, "packages/shared", "shared");
        let scan = find_spfx_solutions(&b.0, 3).unwrap();
        let names: Vec<_> = scan.solutions.iter().map(|s| s.package_name.clone().unwrap()).collect();
        assert_eq!(names, ["intranet", "web", "shared"]);
        assert_eq!(scan.solutions.len(), 3);
        assert!(scan.problems.is_empty());

        // too deep for the limit
        assert_eq!(find_spfx_solutions(&b.0, 1).unwrap().solutions.len(), 0);
        assert_eq!(find_spfx_solutions(&b.0, 2).unwrap().solutions.len(), 3);
    }

    #[test]
    fn the_search_skips_dependencies_hidden_folders_and_what_is_inside_a_solution() {
        let t = Tmp::new();
        solution(&t, "app", "app");
        solution(&t, "app/node_modules/dep", "dep"); // inside the solution: not looked at
        solution(&t, "node_modules/other", "other");
        solution(&t, ".hidden/x", "hidden");
        solution(&t, "lib/y", "y");
        let scan = find_spfx_solutions(&t.0, 6).unwrap();
        assert_eq!(scan.solutions.len(), 1);
        assert_eq!(scan.solutions[0].package_name.as_deref(), Some("app"));
    }

    #[test]
    fn an_unreadable_solution_is_reported_and_does_not_hide_the_others() {
        let t = Tmp::new();
        solution(&t, "good", "good");
        solution(&t, "bad", "bad");
        t.write("bad/package.json", "{ broken");
        let scan = find_spfx_solutions(&t.0, 2).unwrap();
        assert_eq!(scan.solutions.len(), 1);
        assert_eq!(scan.problems.len(), 1, "{:?}", scan.problems);
        assert!(scan.problems[0].contains("package.json"), "{:?}", scan.problems);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        let t = Tmp::new();
        assert!(find_spfx_solutions(&t.0.join("nope"), 2).is_err());
    }
}
