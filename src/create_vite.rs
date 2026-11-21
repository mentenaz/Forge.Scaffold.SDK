//! Reader for the templates of `create-vite` (`packages/create-vite` of `vitejs/vite`, MIT licensed).
//!
//! A create-vite template is a plain folder (`template-react-ts`, `template-vue-ts`, ...). The scaffolder
//! copies it and changes four things, and this reader does the same once, when the template is loaded, so
//! that everything after it (`plan()`, `apply()`, releases) is the ordinary pipeline:
//!
//! * `package.json`: its `name` becomes the project name (a `{__projectName__}` token here);
//! * `index.html`: the text inside `<title>…</title>` becomes the project name;
//! * `_gitignore` is written as `.gitignore` and `_oxlintrc.json` as `.oxlintrc.json` (the manifest's
//!   `rename`; a template repository cannot hold a real `.gitignore` for the project it makes);
//! * the project name is checked as an npm package name (`"rule": "package-name"`).
//!
//! Not done: what create-vite does after copying, because it is code and not data: the options that patch
//! in the React Compiler or ESLint, the `custom-*` choices that hand over to other CLIs (Nuxt, React Router,
//! ...), and installing and starting. Installing comes back as the usual `install` step for the host.
//!
//! Anything this reader does not expect is an error and not a guess: a file that starts with `_` and has no
//! rename, a `package.json` without a `name`, or file text that already looks like one of our tokens.

use crate::error::{Result, ScaffoldError};
use crate::manifest::Manifest;
use crate::source::TemplateSource;
use crate::upstream::{Overlay, TEMPLATE_FOLDER};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Where `create-vite` is, relative to a checkout of the Vite repo.
pub const CREATE_VITE_DIR: &str = "packages/create-vite";
/// The licence that covers the templates.
pub const CREATE_VITE_LICENSE: &str = "packages/create-vite/LICENSE";

/// What create-vite writes under another name (its `renameFiles`). The manifest carries the same map.
const RENAMES: &[(&str, &str)] = &[("_gitignore", ".gitignore"), ("_oxlintrc.json", ".oxlintrc.json")];

fn bad(path: &str, reason: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Template { path: path.to_string(), reason: reason.into() }
}

/// What is known about one create-vite template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteTemplate {
    /// The template's name in create-vite, e.g. `react-ts`.
    pub template: String,
    /// The version of `create-vite` the template comes from (its `package.json`).
    pub version: String,
    /// `engines.node` of create-vite, which the templates' tooling needs.
    pub node: Option<String>,
}

impl ViteTemplate {
    /// The generator's id: `vite-react-ts`.
    pub fn id(&self) -> String {
        format!("vite-{}", self.template)
    }

    pub fn manifest(&self) -> Result<Manifest> {
        Manifest::from_json(&self.manifest_json())
    }

    /// The text of [`ViteTemplate::manifest`], for writing a `manifest.json`.
    pub fn manifest_json(&self) -> String {
        let rename: serde_json::Map<String, Value> = RENAMES.iter().map(|(from, to)| (from.to_string(), json!(to))).collect();
        let mut value = json!({
            "id": self.id(),
            "version": self.version,
            "prompts": [
                { "key": "projectName", "type": "text", "label": "Project name", "default": "vite-project", "rule": "package-name" },
                { "key": "install", "type": "bool", "label": "Run install afterwards", "default": false }
            ],
            "folderFrom": "projectName",
            "include": [ { "folder": TEMPLATE_FOLDER, "rename": rename } ],
            "post": [ { "action": "install", "when": { "key": "install", "equals": true } } ],
            "packageManagers": { "default": "npm", "tested": ["npm"] }
        });
        if let Some(node) = &self.node {
            value["requires"] = json!({ "node": node });
        }
        let mut out = serde_json::to_string_pretty(&value).expect("a json! value always serializes");
        out.push('\n');
        out
    }
}

/// The files of one create-vite template, translated, served as a [`TemplateSource`] with a single folder
/// named [`TEMPLATE_FOLDER`].
#[derive(Debug, Clone)]
pub struct ViteSource {
    files: BTreeMap<String, Vec<u8>>,
}

impl ViteSource {
    /// Reads and translates `template-<template>` from the create-vite folder `dir` of `source` (usually
    /// [`CREATE_VITE_DIR`] of a checkout of the Vite repo).
    pub fn load(source: &dyn TemplateSource, dir: &str, template: &str) -> Result<(ViteTemplate, ViteSource)> {
        if template.is_empty() || !template.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(bad(template, "a create-vite template name is lowercase letters, digits and '-'"));
        }
        let dir = dir.trim_matches('/');
        let join = |p: &str| if dir.is_empty() { p.to_string() } else { format!("{dir}/{p}") };

        let package_path = join("package.json");
        let package: Value = serde_json::from_str(&source.read_text(&package_path)?)
            .map_err(|e| bad(&package_path, format!("not valid JSON: {e}")))?;
        let version = package
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| bad(&package_path, "create-vite's package.json has no version"))?
            .to_string();
        let node = package.pointer("/engines/node").and_then(Value::as_str).map(str::to_string);

        let folder = join(&format!("template-{template}"));
        let listed = source.list_files(&folder)?;
        if listed.is_empty() {
            return Err(bad(&folder, "the template has no files"));
        }

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for rel in listed {
            let full = format!("{folder}/{rel}");
            let name = rel.rsplit('/').next().unwrap_or(&rel);
            if name.starts_with('_') && !RENAMES.iter().any(|(from, _)| *from == name) {
                return Err(bad(
                    &full,
                    "a file named with a leading '_' that create-vite may rename, and this reader does not know a \
                     rename for it: check what create-vite does with it",
                ));
            }

            let bytes = source.read_file(&full)?;
            let bytes = match String::from_utf8(bytes) {
                Ok(text) => {
                    if text.contains("{__") {
                        return Err(bad(&full, "the text contains '{__', which the engine would read as a token"));
                    }
                    let text = match rel.as_str() {
                        "package.json" => set_package_name(&text).map_err(|reason| bad(&full, reason))?,
                        "index.html" => set_title(&text),
                        _ => text,
                    };
                    text.into_bytes()
                }
                Err(e) => e.into_bytes(), // an image or another binary file: copied as it is
            };
            files.insert(rel, bytes);
        }
        if !files.contains_key("package.json") {
            return Err(bad(&folder, "the template has no package.json"));
        }

        Ok((ViteTemplate { template: template.to_string(), version, node }, ViteSource { files }))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Changes the template with `overlay` (your own favicon, page and README, say): a rule replaces every file
    /// whose path is the rule's path or ends with `/<rule path>`, and a removal leaves such files out. A rule
    /// that matches nothing in this template is fine, because one overlay serves many templates. A rule or
    /// removal that starts with `<template>:` (as in `react-ts:src/App.tsx`) applies to that create-vite
    /// template only: `src/App.tsx` means something else in Solid or Preact. `template` is the name of the
    /// template being loaded. Returns how many files were replaced or removed. The text of a replacement
    /// may use the engine's tokens, such as `{__projectName__}`, because it is rendered like any other file.
    pub fn apply_overlay(&mut self, template: &str, overlay: &Overlay) -> usize {
        // The part after the `<template>:` prefix, if the rule is for this template (or for all of them).
        let scoped = |rule: &str| -> Option<String> {
            match rule.split_once(':') {
                Some((name, path)) if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') => {
                    (name == template).then(|| path.to_string())
                }
                _ => Some(rule.to_string()),
            }
        };
        let matches = |path: &str, pattern: &str| path == pattern || path.ends_with(&format!("/{pattern}"));

        let mut changed = 0;
        for (rule, bytes) in overlay.rules() {
            let Some(pattern) = scoped(rule) else { continue };
            for (path, content) in self.files.iter_mut() {
                if matches(path, &pattern) {
                    *content = bytes.clone();
                    changed += 1;
                }
            }
        }
        for rule in overlay.removals() {
            let Some(pattern) = scoped(rule) else { continue };
            let before = self.files.len();
            self.files.retain(|path, _| !matches(path, &pattern));
            changed += before - self.files.len();
        }
        changed
    }
}

const PROJECT_NAME: &str = "{__projectName__}";

/// Sets `name`, the way create-vite does (`pkg.name = packageName`, written with 2 spaces and a final newline).
fn set_package_name(text: &str) -> std::result::Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let object = value.as_object_mut().ok_or("the top level is not an object")?;
    if !object.contains_key("name") {
        return Err("the package.json has no \"name\" to replace".into());
    }
    object.insert("name".into(), json!(PROJECT_NAME));
    let mut out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

/// Replaces the first `<title>…</title>` whose text is on one line, as create-vite's regex does.
fn set_title(html: &str) -> String {
    const OPEN: &str = "<title>";
    const CLOSE: &str = "</title>";
    let Some(start) = html.find(OPEN) else { return html.to_string() };
    let from = start + OPEN.len();
    let Some(len) = html[from..].find(CLOSE) else { return html.to_string() };
    if html[from..from + len].contains('\n') {
        return html.to_string();
    }
    format!("{}{PROJECT_NAME}{}", &html[..from], &html[from + len..])
}

impl TemplateSource for ViteSource {
    fn list_files(&self, folder: &str) -> Result<Vec<String>> {
        if folder != TEMPLATE_FOLDER {
            return Err(bad(folder, "no such template folder"));
        }
        Ok(self.files.keys().cloned().collect())
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        path.strip_prefix(TEMPLATE_FOLDER)
            .and_then(|p| p.strip_prefix('/'))
            .and_then(|rel| self.files.get(rel))
            .cloned()
            .ok_or_else(|| bad(path, "no such template file"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{apply, plan, Answers, PlanOptions};
    use crate::source::DirSource;
    use std::fs;
    use std::path::PathBuf;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-vite-{}", uuid::Uuid::new_v4()));
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

    /// A tiny create-vite: its package.json and one template.
    fn create_vite() -> Tmp {
        let t = Tmp::new();
        t.write("packages/create-vite/package.json", "{ \"name\": \"create-vite\", \"version\": \"9.2.1\", \"engines\": { \"node\": \"^20.19.0 || >=22.12.0\" } }\n");
        t.write("packages/create-vite/LICENSE", "MIT License\n\nCopyright (c) test\n");
        let tpl = "packages/create-vite/template-react-ts";
        t.write(&format!("{tpl}/package.json"), "{\n  \"name\": \"vite-react-typescript-starter\",\n  \"private\": true,\n  \"version\": \"0.0.0\",\n  \"scripts\": {\n    \"dev\": \"vite\"\n  }\n}\n");
        t.write(&format!("{tpl}/index.html"), "<html>\n  <head>\n    <title>Vite + React + TS</title>\n  </head>\n</html>\n");
        t.write(&format!("{tpl}/_gitignore"), "node_modules\n");
        t.write(&format!("{tpl}/_oxlintrc.json"), "{}\n");
        t.write(&format!("{tpl}/src/main.tsx"), "console.log('hi {not a token}');\n");
        t.write(&format!("{tpl}/public/favicon.svg"), "<svg/>\n");
        t
    }

    fn load(t: &Tmp) -> (ViteTemplate, ViteSource) {
        ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "react-ts").unwrap()
    }

    #[test]
    fn the_template_is_translated_the_way_create_vite_changes_it() {
        let t = create_vite();
        let (def, source) = load(&t);
        assert_eq!((def.id().as_str(), def.version.as_str()), ("vite-react-ts", "9.2.1"));
        assert_eq!(def.node.as_deref(), Some("^20.19.0 || >=22.12.0"));

        let read = |p: &str| String::from_utf8(source.read_file(&format!("template/{p}")).unwrap()).unwrap();
        assert_eq!(
            read("package.json"),
            "{\n  \"name\": \"{__projectName__}\",\n  \"private\": true,\n  \"version\": \"0.0.0\",\n  \"scripts\": {\n    \"dev\": \"vite\"\n  }\n}\n",
            "the name changes and nothing else, in the same order"
        );
        assert!(read("index.html").contains("<title>{__projectName__}</title>"));
        assert_eq!(read("_gitignore"), "node_modules\n", "the file keeps its name here; the manifest renames it when copying");
        assert_eq!(read("src/main.tsx"), "console.log('hi {not a token}');\n");
        assert_eq!(source.file_count(), 6);
    }

    #[test]
    fn scaffolding_writes_the_project_with_dotfiles_and_the_chosen_name() {
        let t = create_vite();
        let (def, source) = load(&t);
        let loc = Tmp::new();
        let answers: Answers = serde_json::from_value(json!({ "projectName": "my.app" })).unwrap();
        let planned = plan(&def.manifest().unwrap(), &source, &answers, PlanOptions::new(&loc.0)).unwrap();
        apply(&planned, |_| {}).unwrap();

        let root = loc.0.join("my.app");
        assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), "node_modules\n");
        assert_eq!(fs::read_to_string(root.join(".oxlintrc.json")).unwrap(), "{}\n");
        assert!(!root.join("_gitignore").exists() && !root.join("_oxlintrc.json").exists());
        assert!(fs::read_to_string(root.join("package.json")).unwrap().contains("\"name\": \"my.app\""));
        assert!(fs::read_to_string(root.join("index.html")).unwrap().contains("<title>my.app</title>"));
        assert!(root.join("public/favicon.svg").exists());
    }

    #[test]
    fn the_project_name_is_checked_as_a_package_name_and_the_install_step_is_a_descriptor() {
        let t = create_vite();
        let (def, source) = load(&t);
        let loc = Tmp::new();
        let m = def.manifest().unwrap();
        let bad_name: Answers = serde_json::from_value(json!({ "projectName": "My App" })).unwrap();
        assert!(plan(&m, &source, &bad_name, PlanOptions::new(&loc.0)).is_err());
        let ok: Answers = serde_json::from_value(json!({ "projectName": "my-app", "install": true })).unwrap();
        let planned = plan(&m, &source, &ok, PlanOptions::new(&loc.0)).unwrap();
        assert_eq!(planned.post.len(), 1);
        assert_eq!(def.manifest().unwrap().node_check("v22.14.0").unwrap(), crate::node_range::NodeCheck::Fine);
        assert!(matches!(def.manifest().unwrap().node_check("v18.20.8").unwrap(), crate::node_range::NodeCheck::Mismatch { .. }));
    }

    #[test]
    fn what_the_reader_does_not_expect_is_refused() {
        let t = create_vite();
        t.write("packages/create-vite/template-react-ts/_mystery", "x");
        let err = ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "react-ts").unwrap_err().to_string();
        assert!(err.contains("leading '_'"), "{err}");

        let t = create_vite();
        t.write("packages/create-vite/template-react-ts/src/a.ts", "const x = '{__projectName__}';\n");
        assert!(ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "react-ts").unwrap_err().to_string().contains("token"));

        let t = create_vite();
        t.write("packages/create-vite/template-react-ts/package.json", "{ \"private\": true }\n");
        assert!(ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "react-ts").unwrap_err().to_string().contains("no \"name\""));

        assert!(ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "nope").is_err());
        assert!(ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "../x").is_err());
        assert!(ViteSource::load(&DirSource::new(&t.0), CREATE_VITE_DIR, "").is_err());
    }

    #[test]
    fn an_overlay_replaces_images_by_the_end_of_their_path_and_ignores_rules_that_match_nothing() {
        let t = create_vite();
        let (_, mut source) = load(&t);
        let overlay_dir = Tmp::new();
        overlay_dir.write(
            "overlay.json",
            r#"{ "replace": { "public/favicon.svg": "mark.svg", "main.tsx": "other.txt", "assets/welcome-dark.png": "other.txt",
                              "react-ts:index.html": "other.txt", "vue-ts:src/main.tsx": "mark.svg" },
                 "remove": ["react-ts:public/favicon.svg", "solid-ts:src/main.tsx", "nothing/here.txt"] }"#,
        );
        overlay_dir.write("mark.svg", "<svg>forge</svg>");
        overlay_dir.write("other.txt", "other");
        let overlay = Overlay::load(&DirSource::new(&overlay_dir.0), "").unwrap();

        let replaced = source.apply_overlay("react-ts", &overlay);
        let read = |p: &str| String::from_utf8(source.read_file(&format!("template/{p}")).unwrap()).unwrap();
        // replaced: favicon.svg, src/main.tsx and (react-ts only) index.html; removed: public/favicon.svg
        assert_eq!(replaced, 4, "the welcome-dark, vue-ts and solid-ts rules match nothing here");
        assert_eq!(read("src/main.tsx"), "other", "a rule matches at the end of the path");
        assert_eq!(read("index.html"), "other", "a rule for this template applies");
        assert!(source.read_file("template/public/favicon.svg").is_err(), "a removal leaves the file out");
        assert!(source.list_files("template").unwrap().iter().all(|f| f != "public/favicon.svg"));
        assert!(read("package.json").contains("{__projectName__}"), "other files are untouched");

        // the same rules for another template: only the unscoped ones apply
        let (_, mut other) = load(&t);
        assert_eq!(other.apply_overlay("vue-ts", &overlay), 3, "favicon.svg and main.tsx replaced, then vue-ts's main.tsx rule");

    }

    #[test]
    fn the_title_is_replaced_only_when_it_is_on_one_line() {
        assert_eq!(set_title("<title>Old</title>"), "<title>{__projectName__}</title>");
        assert_eq!(set_title("<head></head>"), "<head></head>");
        let multi = "<title>\n  Old\n</title>";
        assert_eq!(set_title(multi), multi, "create-vite's regex does not match across lines either");
        assert_eq!(set_title("<title>A</title><title>B</title>"), "<title>{__projectName__}</title><title>B</title>");
    }
}
