//! Reader for the template format of the official SharePoint Framework CLI
//! (`SharePoint/spfx`, MIT licensed, Microsoft).
//!
//! An upstream template is a folder with a small `template.json` and the files
//! of a whole solution, where
//!
//! * file contents hold EJS tags like `<%= componentName.pascal %>`, and
//! * file and folder names hold placeholders like `{componentName.camel}`.
//!
//! The upstream templates only ever use plain `<%= name %>` / `<%= name.casing %>`
//! substitutions, so this reader does not run EJS. It translates each tag and
//! placeholder into this crate's `{__token__}` form once, when the template is
//! loaded, and everything after that (`plan()`, `apply()`, packs, releases) is
//! the ordinary pipeline. Any other EJS tag (`<% if %>`, `<%- %>`, ...) is an
//! error, so a template that needs more than substitution is refused instead of
//! rendered wrongly.
//!
//! What the translation covers:
//!
//! * casings `pascal`, `camel`, `hyphen` (our `Kebab`) and `allCaps`;
//! * the built-in values: `componentName`, `libraryName`, `solutionName`
//!   (also written `solution_name`), `componentAlias`, `componentDescription`,
//!   the GUIDs `componentId`, `featureId`, `solutionId`, and the constants
//!   `spfxVersion` and `spfxVersionForBadgeUrl` (taken from `template.json`);
//! * the `_phase:*` scripts that upstream strips from `package.json`.
//!
//! One difference from upstream: its `<%= %>` escapes HTML (`'` becomes `&#39;`).
//! Values are inserted as typed here, so a quote in a description arrives as a
//! quote.

use crate::error::{Result, ScaffoldError};
use crate::manifest::Manifest;
use crate::plan::Answers;
use crate::source::TemplateSource;
use crate::tokens::to_kebab;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Name of the one folder an [`UpstreamSource`] serves: the translated files.
pub const TEMPLATE_FOLDER: &str = "template";
const TEMPLATE_JSON: &str = "template.json";

/// Variables that are text answers. They get all casings.
const TEXT_VARS: &[&str] = &[
    "componentName",
    "libraryName",
    "solutionName",
    "componentAlias",
    "componentDescription",
];
/// Variables that are generated GUIDs. They have no casings.
const GUID_VARS: &[&str] = &["componentId", "featureId", "solutionId"];

/// Files that are never read as text (upstream skips EJS for these too).
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "ico", "bmp", "webp", "woff", "woff2", "ttf", "eot", "otf",
    "mp3", "mp4", "webm", "wav", "zip", "pdf",
];

/// The contents of `template.json`. Fields this reader does not know are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamTemplate {
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Version of the template itself (not of SPFx).
    pub version: String,
    /// The SPFx version the template targets, e.g. `1.24.1`.
    pub spfx_version: String,
    #[serde(default)]
    pub minimum_engine_version: Option<String>,
    /// `engines.node` of the template's `package.json`, read when the template is
    /// loaded. Not part of `template.json`.
    #[serde(skip)]
    pub node: Option<String>,
}

/// What an expression resolves to while translating.
enum Piece {
    Literal(String),
    Token(String),
}

fn bad(path: &str, reason: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Template {
        path: path.to_string(),
        reason: reason.into(),
    }
}

impl UpstreamTemplate {
    /// Resolves `name` / `name.casing` the way upstream's render context does.
    fn resolve(&self, name: &str, casing: Option<&str>) -> std::result::Result<Piece, String> {
        let name = if name == "solution_name" { "solutionName" } else { name };
        match name {
            "spfxVersion" | "spfxVersionForBadgeUrl" => {
                if casing.is_some() {
                    return Err(format!("'{name}' has no casings"));
                }
                Ok(Piece::Literal(if name == "spfxVersion" {
                    self.spfx_version.clone()
                } else {
                    self.spfx_version.replace('-', "--")
                }))
            }
            n if GUID_VARS.contains(&n) => match casing {
                None => Ok(Piece::Token(n.to_string())),
                Some(_) => Err(format!("'{n}' has no casings")),
            },
            n if TEXT_VARS.contains(&n) => {
                let suffix = match casing {
                    None => "",
                    Some("pascal") => "Pascal",
                    Some("camel") => "Camel",
                    Some("hyphen") => "Kebab",
                    Some("allCaps") => "AllCaps",
                    Some(other) => return Err(format!("unknown casing '{other}' on '{n}'")),
                };
                Ok(Piece::Token(format!("{n}{suffix}")))
            }
            other => Err(format!("unknown variable '{other}'")),
        }
    }

    /// `<%= expr %>` bodies and `{expr}` placeholders share this grammar:
    /// an identifier, optionally followed by `.casing`.
    fn resolve_expr(&self, expr: &str) -> std::result::Result<Piece, String> {
        let (name, casing) = match expr.split_once('.') {
            Some((n, c)) => (n, Some(c)),
            None => (expr, None),
        };
        let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ident(name) || casing.is_some_and(|c| !ident(c)) {
            return Err(format!("'{expr}' is not a plain variable"));
        }
        self.resolve(name, casing)
    }

    /// Translates the EJS tags in a text file into `{__token__}`s.
    fn translate_content(&self, text: &str) -> std::result::Result<String, String> {
        if text.contains("{__") {
            return Err("contains '{__', which would be read as a token".into());
        }
        if text.contains("%%>") {
            return Err("contains '%%>' (an escaped EJS close tag), which is not supported".into());
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(i) = rest.find("<%") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            if let Some(r) = after.strip_prefix('%') {
                out.push_str("<%"); // `<%%` is a literal `<%`
                rest = r;
                continue;
            }
            let Some(body) = after.strip_prefix('=') else {
                let shown: String = rest[i..].chars().take(24).collect();
                return Err(format!(
                    "unsupported EJS tag near '{shown}': only <%= name %> is supported"
                ));
            };
            let end = body.find("%>").ok_or("a <%= tag is never closed")?;
            match self.resolve_expr(body[..end].trim())? {
                Piece::Literal(v) => out.push_str(&v),
                Piece::Token(t) => {
                    out.push_str("{__");
                    out.push_str(&t);
                    out.push_str("__}");
                }
            }
            rest = &body[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Translates `{componentName.camel}` placeholders in a path. Braces that
    /// are not a known placeholder are left as they are.
    fn translate_path(&self, path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        let mut rest = path;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let piece = after
                .find('}')
                .and_then(|end| self.resolve_expr(&after[..end]).ok().map(|p| (p, end)));
            match piece {
                Some((p, end)) => {
                    match p {
                        Piece::Literal(v) => out.push_str(&v),
                        Piece::Token(t) => {
                            out.push_str("{__");
                            out.push_str(&t);
                            out.push_str("__}");
                        }
                    }
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The manifest that drives `plan()` for this template.
    ///
    /// Upstream takes these inputs on the command line, so they become prompts:
    /// `componentName`, `libraryName`, `solutionName` (the new folder),
    /// `componentAlias`, `componentDescription`, plus `install`. Use
    /// [`UpstreamInputs`] to fill the ones a caller leaves out the way
    /// upstream does.
    pub fn manifest(&self) -> Result<Manifest> {
        Manifest::from_json(&self.manifest_json())
    }

    /// The text of [`UpstreamTemplate::manifest`], for writing a `manifest.json`.
    pub fn manifest_json(&self) -> String {
        let value = json!({
            "id": self.name,
            "version": self.spfx_version,
            "prompts": [
                { "key": "componentName", "type": "text", "label": "Component name" },
                { "key": "libraryName", "type": "text", "label": "Library name" },
                { "key": "solutionName", "type": "text", "label": "Solution name", "rule": "solution-name" },
                { "key": "componentAlias", "type": "text", "label": "Component alias" },
                { "key": "componentDescription", "type": "text", "label": "Component description" },
                { "key": "install", "type": "bool", "label": "Run install afterwards", "default": false }
            ],
            "folderFrom": "solutionName",
            "nestedMarker": "config/package-solution.json",
            "guids": ["componentId", "featureId", "solutionId"],
            "include": [ { "folder": TEMPLATE_FOLDER } ],
            "post": [ { "action": "install", "when": { "key": "install", "equals": true } } ],
            "packageManagers": { "default": "npm", "tested": ["npm"] },
            "add": {
                "mode": "render",
                "buildSystem": "heft",
                "component": "componentName",
                "inherit": { "libraryName": "packageName", "solutionName": "packageName" },
                "merge": {
                    "package.json": "package-json",
                    "config/config.json": "spfx-config",
                    "config/package-solution.json": "spfx-package-solution",
                    "config/serve.json": "spfx-serve",
                    "sharepoint/assets/elements.xml": "spfx-elements",
                    "sharepoint/assets/ClientSideInstance.xml": "spfx-elements"
                },
                "exclusive": ["config/copilot-agent.json"]
            }
        });
        let mut value = value;
        if let Some(node) = &self.node {
            value["requires"] = json!({ "node": node });
        }
        let mut out = serde_json::to_string_pretty(&value).expect("a json! value always serializes");
        out.push('\n');
        out
    }
}

/// Removes the `_phase:*` scripts, which upstream only uses to build its own
/// templates repo. Returns the text unchanged when there are none.
fn strip_phase_scripts(text: &str) -> std::result::Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let Some(scripts) = value.get_mut("scripts").and_then(Value::as_object_mut) else {
        return Ok(text.to_string());
    };
    let before = scripts.len();
    scripts.retain(|k, _| !k.starts_with("_phase:"));
    if scripts.len() == before {
        return Ok(text.to_string());
    }
    let mut out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

fn is_binary_name(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| BINARY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The files of one upstream template, translated, served as a
/// [`TemplateSource`] with a single folder named [`TEMPLATE_FOLDER`].
#[derive(Debug, Clone)]
pub struct UpstreamSource {
    files: BTreeMap<String, Vec<u8>>,
}

impl UpstreamSource {
    /// Reads and translates the template in folder `dir` of `source` (`""` when
    /// `source` is rooted at the template itself).
    pub fn load(source: &dyn TemplateSource, dir: &str) -> Result<(UpstreamTemplate, UpstreamSource)> {
        let dir = dir.trim_matches('/');
        let join = |p: &str| {
            if dir.is_empty() {
                p.to_string()
            } else {
                format!("{dir}/{p}")
            }
        };

        let json_path = join(TEMPLATE_JSON);
        let mut template: UpstreamTemplate = serde_json::from_str(&source.read_text(&json_path)?)
            .map_err(|e| bad(&json_path, format!("invalid template.json: {e}")))?;

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut node = None;
        for rel in source.list_files(dir)? {
            if rel == TEMPLATE_JSON {
                continue;
            }
            let full = join(&rel);
            let bytes = source.read_file(&full)?;

            let translated = template.translate_path(&rel);
            let bytes = match (is_binary_name(&rel), String::from_utf8(bytes)) {
                (false, Ok(text)) => {
                    let mut text = template
                        .translate_content(&text)
                        .map_err(|reason| bad(&full, reason))?;
                    if translated == "package.json" || translated.ends_with("/package.json") {
                        text = strip_phase_scripts(&text).map_err(|reason| bad(&full, reason))?;
                    }
                    if translated == "package.json" {
                        node = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| v.pointer("/engines/node").and_then(Value::as_str).map(str::to_string));
                    }
                    text.into_bytes()
                }
                (_, Ok(text)) => text.into_bytes(),
                (_, Err(e)) => e.into_bytes(),
            };

            if files.insert(translated.clone(), bytes).is_some() {
                return Err(bad(&full, format!("two files would be written to '{translated}'")));
            }
        }
        template.node = node;
        Ok((template, UpstreamSource { files }))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Applies the `rewrite` rules of `overlay` that name this template: every occurrence of the rule's `from` in a file
    /// path and in the text of a file becomes `to`. Both are written in the template's own placeholder syntax
    /// (`{componentName.camel}WebPart/`), so they are translated the way paths are. Returns how many files changed (their
    /// path, their text, or both). A rewrite that would put two files at one path is an error and the source is left
    /// unusable: the caller stops there.
    pub fn apply_rewrites(&mut self, template: &UpstreamTemplate, overlay: &Overlay) -> Result<usize> {
        let mut changed = 0;
        for rule in overlay.rewrite.iter().filter(|r| r.templates.contains(&template.name)) {
            let from = template.translate_path(&rule.from);
            let to = template.translate_path(&rule.to);
            let mut next: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            for (path, content) in std::mem::take(&mut self.files) {
                let new_path = path.replace(&from, &to);
                let mut touched = new_path != path;
                let content = match std::str::from_utf8(&content) {
                    Ok(text) if !is_binary_name(&path) && text.contains(&from) => {
                        touched = true;
                        text.replace(&from, &to).into_bytes()
                    }
                    _ => content,
                };
                if touched {
                    changed += 1;
                }
                if next.insert(new_path.clone(), content).is_some() {
                    return Err(bad(&new_path, format!("the overlay rewrite '{}' would put two files at this path", rule.from)));
                }
            }
            self.files = next;
        }
        Ok(changed)
    }

    /// Replaces files of the template with the files of `overlay` (your own
    /// logo and icons, say). A rule replaces every file whose path is the rule's
    /// path or ends with `/<rule path>`; a rule that matches nothing in this
    /// template is fine, because one overlay serves many templates. Returns how
    /// many files were replaced.
    pub fn apply_overlay(&mut self, template: &UpstreamTemplate, overlay: &Overlay) -> usize {
        let mut replaced = 0;
        for (pattern, bytes) in &overlay.replace {
            let pattern = template.translate_path(pattern);
            let suffix = format!("/{pattern}");
            for (path, content) in self.files.iter_mut() {
                if *path == pattern || path.ends_with(&suffix) {
                    *content = bytes.clone();
                    replaced += 1;
                }
            }
        }
        replaced
    }
}

/// Files that replace files of upstream templates, described by an
/// `overlay.json` next to them:
///
/// ```json
/// { "replace": {
///     "assets/welcome-dark.png": "assets/common/logo.png",
///     "teams/{componentId}_color.png": "assets/common/teams-color-192.png" } }
/// ```
///
/// Each key is a path in the upstream template's own syntax, matched at the end
/// of a file's path (so `assets/welcome-dark.png` also finds
/// `src/webparts/x/assets/welcome-dark.png`). Each value is a file relative to
/// the overlay folder.
///
/// A `"rewrite"` list changes the text of some official templates, each rule with a `"why"`:
///
/// ```json
/// { "replace": {},
///   "rewrite": [ { "templates": ["webpart-react"],
///                  "from": "{componentName.camel}WebPart/", "to": "{componentName.camel}/",
///                  "why": "our web part folders have no WebPart suffix" } ] }
/// ```
///
/// Every occurrence of `from` in a path and in the text of a file becomes `to`, so the places that name a renamed folder
/// follow it. This is how a change to Microsoft's own layout stays one marked line, to be merged by hand when upstream moves.
///
/// Two things only the create-vite templates use (see [`crate::create_vite`]; the SPFx templates ignore them):
/// a key or a removal may start with `<template>:`, as in `react-ts:src/App.tsx`, and then applies to that
/// create-vite template only; and `"remove"` lists paths to leave out of the template altogether.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    replace: Vec<(String, Vec<u8>)>,
    remove: Vec<String>,
    rewrite: Vec<Rewrite>,
}

/// A text change the overlay makes to some of the official templates, with the reason it is made. See [`Overlay`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rewrite {
    /// The names of the templates it applies to (`webpart-react`).
    templates: Vec<String>,
    /// What to find, in a path or in the text of a file, in the template's own syntax (`{componentName.camel}WebPart/`).
    from: String,
    /// What it becomes (`{componentName.camel}/`).
    to: String,
    /// Why, for whoever merges a later upstream version by hand. Required, so no rewrite is left unexplained.
    why: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayJson {
    replace: BTreeMap<String, String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    rewrite: Vec<Rewrite>,
}

impl Overlay {
    /// The replacement rules: a path in the template's own syntax and the bytes that replace the file.
    pub(crate) fn rules(&self) -> &[(String, Vec<u8>)] {
        &self.replace
    }

    /// The paths to leave out.
    pub(crate) fn removals(&self) -> &[String] {
        &self.remove
    }

    /// Reads `overlay.json` in folder `dir` of `source` and the files it names.
    pub fn load(source: &dyn TemplateSource, dir: &str) -> Result<Overlay> {
        let dir = dir.trim_matches('/');
        let join = |p: &str| {
            if dir.is_empty() {
                p.to_string()
            } else {
                format!("{dir}/{p}")
            }
        };
        let json_path = join("overlay.json");
        let parsed: OverlayJson = serde_json::from_str(&source.read_text(&json_path)?)
            .map_err(|e| bad(&json_path, format!("invalid overlay.json: {e}")))?;
        let mut replace = Vec::with_capacity(parsed.replace.len());
        for (pattern, file) in parsed.replace {
            let file = crate::stage::normalize_rel(&file)?;
            replace.push((pattern, source.read_file(&join(&file))?));
        }
        for r in &parsed.rewrite {
            let safe = |p: &str| !p.starts_with('/') && !p.contains('\\') && !p.split('/').any(|s| s == "..");
            if r.templates.is_empty() || r.from.is_empty() || r.from == r.to || r.why.trim().is_empty() || !safe(&r.to) {
                return Err(bad(
                    &json_path,
                    format!("the rewrite of '{}' needs templates, a from that changes into a different relative to, and a why", r.from),
                ));
            }
        }
        Ok(Overlay { replace, remove: parsed.remove, rewrite: parsed.rewrite })
    }
}

impl TemplateSource for UpstreamSource {
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

/// What a caller knows when it starts an upstream template. Everything except
/// the component name is optional and defaults the way upstream's CLI does.
#[derive(Debug, Clone, Default)]
pub struct UpstreamInputs {
    /// Human-readable name, e.g. `Hello World`.
    pub component_name: String,
    /// Defaults to the solution name.
    pub library_name: Option<String>,
    /// The new folder. Defaults to the component name in hyphen-case.
    pub solution_name: Option<String>,
    /// Defaults to the component name.
    pub component_alias: Option<String>,
    /// Defaults to `<component name> description`.
    pub component_description: Option<String>,
    pub install: bool,
}

impl UpstreamInputs {
    /// The answers for adding a component to an existing solution: the solution
    /// supplies its own name and library name (the manifest's `add.inherit`), so
    /// only the component's own answers are given, plus a library or solution
    /// name if the caller set one explicitly.
    pub fn add_answers(&self) -> Answers {
        let mut a = Answers::new();
        a.insert("componentName".into(), json!(self.component_name));
        a.insert(
            "componentAlias".into(),
            json!(self.component_alias.clone().unwrap_or_else(|| self.component_name.clone())),
        );
        a.insert(
            "componentDescription".into(),
            json!(self
                .component_description
                .clone()
                .unwrap_or_else(|| format!("{} description", self.component_name))),
        );
        a.insert("install".into(), json!(self.install));
        if let Some(v) = &self.library_name {
            a.insert("libraryName".into(), json!(v));
        }
        if let Some(v) = &self.solution_name {
            a.insert("solutionName".into(), json!(v));
        }
        a
    }

    pub fn answers(&self) -> Answers {
        let solution = self
            .solution_name
            .clone()
            .unwrap_or_else(|| to_kebab(&self.component_name));
        let mut a = Answers::new();
        a.insert("componentName".into(), json!(self.component_name));
        a.insert("solutionName".into(), json!(solution));
        a.insert(
            "libraryName".into(),
            json!(self.library_name.clone().unwrap_or_else(|| solution.clone())),
        );
        a.insert(
            "componentAlias".into(),
            json!(self.component_alias.clone().unwrap_or_else(|| self.component_name.clone())),
        );
        a.insert(
            "componentDescription".into(),
            json!(self
                .component_description
                .clone()
                .unwrap_or_else(|| format!("{} description", self.component_name))),
        );
        a.insert("install".into(), json!(self.install));
        a
    }
}

/// The words of an official template's name, as they read in a title.
fn title_word(word: &str) -> &str {
    match word {
        "webpart" => "web part",
        "fieldcustomizer" => "field customizer",
        "formcustomizer" => "form customizer",
        "applicationcustomizer" => "application customizer",
        "listviewcommandset" => "list view command set",
        "primarytext" => "primary text",
        "ace" => "ACE",
        other => other,
    }
}

impl UpstreamTemplate {
    /// What a list of templates shows for this official template: a title made from its name (`webpart-react` is
    /// `Web part (React)`), its own description, and the tags `spfx` and its category. Used by the release builder, which
    /// puts it in the index only: the archive is not changed, so its revision does not move.
    pub(crate) fn display(&self) -> (String, Option<String>, Vec<String>) {
        let words: Vec<&str> = self.name.split('-').collect();
        let (head, framework) = match words.last() {
            Some(&"react") if words.len() > 1 => (&words[..words.len() - 1], Some("React")),
            Some(&"minimal") if words.len() > 1 => (&words[..words.len() - 1], Some("minimal")),
            Some(&"noframework") if words.len() > 1 => (&words[..words.len() - 1], Some("no framework")),
            _ => (&words[..], None),
        };
        let mut title = head.iter().map(|w| title_word(w)).collect::<Vec<_>>().join(" ");
        if let Some(first) = title.chars().next() {
            title = first.to_uppercase().collect::<String>() + &title[first.len_utf8()..];
        }
        if let Some(f) = framework {
            title = format!("{title} ({f})");
        }
        let description = self.description.as_ref().map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        let mut tags = vec!["spfx".to_string()];
        let category: String = self
            .category
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let category = category.trim_matches('-').to_string();
        if !category.is_empty() && !tags.contains(&category) {
            tags.push(category);
        }
        (title, description, tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template held in memory, rooted at the template folder itself.
    struct Mem(BTreeMap<String, Vec<u8>>);
    impl Mem {
        fn new(files: &[(&str, &[u8])]) -> Self {
            Mem(files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect())
        }
    }
    impl TemplateSource for Mem {
        fn list_files(&self, _folder: &str) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
        fn read_file(&self, path: &str) -> Result<Vec<u8>> {
            self.0.get(path).cloned().ok_or_else(|| bad(path, "missing"))
        }
    }

    const TEMPLATE_JSON_TEXT: &str = r#"{
      "name": "demo-part", "category": "webpart", "description": "d",
      "version": "0.0.1", "spfxVersion": "1.24.1", "somethingNew": true
    }"#;

    fn load(files: &[(&str, &[u8])]) -> Result<(UpstreamTemplate, UpstreamSource)> {
        let mut all = vec![("template.json", TEMPLATE_JSON_TEXT.as_bytes())];
        all.extend_from_slice(files);
        UpstreamSource::load(&Mem::new(&all), "")
    }

    fn text(src: &UpstreamSource, rel: &str) -> String {
        String::from_utf8(src.read_file(&format!("{TEMPLATE_FOLDER}/{rel}")).unwrap()).unwrap()
    }

    #[test]
    fn reads_template_json_and_ignores_unknown_fields() {
        let (t, _) = load(&[]).unwrap();
        assert_eq!((t.name.as_str(), t.spfx_version.as_str()), ("demo-part", "1.24.1"));
        assert_eq!(t.category, "webpart");
    }

    #[test]
    fn translates_tags_in_content_with_every_casing() {
        let (_, s) = load(&[(
            "a.ts",
            b"<%= componentName.pascal %>|<%=componentName.camel%>|<%= componentName.hyphen %>|<%= componentName.allCaps %>|<%= componentName %>|<%= solution_name %>|<%= libraryName %>",
        )])
        .unwrap();
        assert_eq!(
            text(&s, "a.ts"),
            "{__componentNamePascal__}|{__componentNameCamel__}|{__componentNameKebab__}|{__componentNameAllCaps__}|{__componentName__}|{__solutionName__}|{__libraryName__}"
        );
    }

    #[test]
    fn constants_come_from_template_json() {
        let (_, s) = load(&[("a", b"v<%= spfxVersion %> b<%= spfxVersionForBadgeUrl %>")]).unwrap();
        assert_eq!(text(&s, "a"), "v1.24.1 b1.24.1");

        let pre = TEMPLATE_JSON_TEXT.replace("1.24.1", "1.24.0-beta.1");
        let mem = Mem::new(&[("template.json", pre.as_bytes()), ("a", b"<%= spfxVersionForBadgeUrl %>")]);
        let (_, s) = UpstreamSource::load(&mem, "").unwrap();
        assert_eq!(text(&s, "a"), "1.24.0--beta.1");
    }

    #[test]
    fn translates_placeholders_in_paths_and_leaves_other_braces() {
        let (_, s) = load(&[
            ("src/{componentName.camel}WebPart/{componentName.pascal}.ts", b"x"),
            ("teams/{componentId}_color.png", &[0x89, 0xff]),
            ("odd/{not a var}.txt", b"y"),
        ])
        .unwrap();
        let files = s.list_files(TEMPLATE_FOLDER).unwrap();
        assert!(files.contains(&"src/{__componentNameCamel__}WebPart/{__componentNamePascal__}.ts".to_string()));
        assert!(files.contains(&"teams/{__componentId__}_color.png".to_string()));
        assert!(files.contains(&"odd/{not a var}.txt".to_string()));
    }

    #[test]
    fn template_json_is_not_part_of_the_output() {
        let (_, s) = load(&[("a", b"x")]).unwrap();
        assert_eq!(s.list_files(TEMPLATE_FOLDER).unwrap(), vec!["a"]);
    }

    #[test]
    fn binary_files_pass_through_untouched() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0xff, b'<', b'%'];
        let (_, s) = load(&[("i.png", &png), ("odd.bin", &[0xff, 0xfe, b'<', b'%', b'='])]).unwrap();
        assert_eq!(s.read_file("template/i.png").unwrap(), png);
        assert_eq!(s.read_file("template/odd.bin").unwrap(), vec![0xff, 0xfe, b'<', b'%', b'=']);
    }

    #[test]
    fn anything_beyond_plain_substitution_is_refused() {
        for body in ["<% if (x) { %>a<% } %>", "<%- componentName %>", "<%# comment %>", "<%= 1 + 2 %>",
                     "<%= nope %>", "<%= componentId.pascal %>", "<%= componentName.shouty %>",
                     "<%= componentName", "x %%> y", "{__componentName__}"] {
            let err = load(&[("a.ts", body.as_bytes())]).unwrap_err().to_string();
            assert!(err.contains("a.ts"), "{body}: {err}");
        }
    }

    #[test]
    fn escaped_open_tag_is_a_literal() {
        let (_, s) = load(&[("a", b"a <%% b")]).unwrap();
        assert_eq!(text(&s, "a"), "a <% b");
    }

    #[test]
    fn phase_scripts_are_stripped_from_package_json_only_when_present() {
        let with = br#"{ "name": "<%= libraryName %>", "scripts": { "build": "x", "_phase:build": "y", "_phase:test": "z" } }"#;
        let without = b"{\"name\":\"a\",\"scripts\":{\"build\":\"x\"}}";
        let (_, s) = load(&[("package.json", with), ("sub/package.json", without)]).unwrap();
        assert_eq!(
            text(&s, "package.json"),
            "{\n  \"name\": \"{__libraryName__}\",\n  \"scripts\": {\n    \"build\": \"x\"\n  }\n}\n"
        );
        assert_eq!(text(&s, "sub/package.json"), String::from_utf8_lossy(without));
    }

    #[test]
    fn manifest_validates_and_answers_default_like_upstream() {
        let (t, _) = load(&[]).unwrap();
        let m = t.manifest().unwrap();
        assert_eq!(m.id, "demo-part");
        assert_eq!(m.version, "1.24.1");

        let a = UpstreamInputs { component_name: "Hello World".into(), ..Default::default() }.answers();
        assert_eq!(a["solutionName"], "hello-world");
        assert_eq!(a["libraryName"], "hello-world");
        assert_eq!(a["componentAlias"], "Hello World");
        assert_eq!(a["componentDescription"], "Hello World description");
    }

    #[test]
    fn overlay_replaces_files_by_path_suffix_and_ignores_rules_that_match_nothing() {
        let (t, mut s) = load(&[
            ("src/{componentName.camel}/assets/welcome-dark.png", &[1, 0xff]),
            ("assets/welcome-dark.png", &[2, 0xff]),
            ("teams/{componentId}_color.png", &[3, 0xff]),
            ("src/other-assets/welcome-dark.png", &[4, 0xff]),
            ("a.ts", b"keep"),
        ])
        .unwrap();
        let overlay = Mem::new(&[
            (
                "overlay.json",
                br#"{ "replace": { "assets/welcome-dark.png": "img/new.png",
                                   "teams/{componentId}_color.png": "img/color.png",
                                   "nothing/here.png": "img/new.png" } }"#,
            ),
            ("img/new.png", &[9, 9]),
            ("img/color.png", &[8, 8]),
        ]);
        let overlay = Overlay::load(&overlay, "").unwrap();
        assert_eq!(s.apply_overlay(&t, &overlay), 3);

        let read = |p: &str| s.read_file(&format!("{TEMPLATE_FOLDER}/{p}")).unwrap();
        assert_eq!(read("src/{__componentNameCamel__}/assets/welcome-dark.png"), vec![9, 9]);
        assert_eq!(read("assets/welcome-dark.png"), vec![9, 9]);
        assert_eq!(read("teams/{__componentId__}_color.png"), vec![8, 8]);
        // `other-assets/...` only ends with the same letters, not with `/assets/...`
        assert_eq!(read("src/other-assets/welcome-dark.png"), vec![4, 0xff]);
        assert_eq!(read("a.ts"), b"keep");
    }

    // ------------------------------------------------- the overlay's rewrite rules

    const REWRITE: &str = r#"{ "replace": {},
        "rewrite": [ { "templates": ["demo-part", "other"], "from": "{componentName.camel}WebPart/", "to": "{componentName.camel}/",
                       "why": "our folders have no suffix" } ] }"#;

    #[test]
    fn a_rewrite_renames_the_folder_and_every_place_that_names_it_and_nothing_else() {
        let (t, mut s) = load(&[
            ("src/webparts/{componentName.camel}WebPart/{componentName.pascal}WebPart.ts", b"export default class <%= componentName.pascal %>WebPart {}"),
            ("src/webparts/{componentName.camel}WebPart/loc/{componentName.camel}WebPartStrings.d.ts", b"declare module '<%= componentName.pascal %>WebPartStrings' {}"),
            ("src/webparts/{componentName.camel}WebPart/{componentName.pascal}WebPart.module.scss", b".<%= componentName.camel %>WebPart { color: red }"),
            (
                "config/config.json",
                br#"{ "entrypoint": "./lib/webparts/<%= componentName.camel %>WebPart/<%= componentName.pascal %>WebPart.js",
                     "strings": "lib/webparts/<%= componentName.camel %>WebPart/loc/{locale}.js" }"#,
            ),
            ("assets/logo.png", &[0xff, 0xfe, b'{', b'_', b'_']),
        ])
        .unwrap();
        let overlay = Overlay::load(&Mem::new(&[("overlay.json", REWRITE.as_bytes())]), "").unwrap();
        // the three files in the folder and config.json change; the picture does not
        assert_eq!(s.apply_rewrites(&t, &overlay).unwrap(), 4);

        let mut paths = s.list_files(TEMPLATE_FOLDER).unwrap();
        paths.sort();
        assert_eq!(
            paths,
            [
                "assets/logo.png",
                "config/config.json",
                "src/webparts/{__componentNameCamel__}/loc/{__componentNameCamel__}WebPartStrings.d.ts",
                "src/webparts/{__componentNameCamel__}/{__componentNamePascal__}WebPart.module.scss",
                "src/webparts/{__componentNameCamel__}/{__componentNamePascal__}WebPart.ts",
            ],
            "the folder lost its suffix; the file names did not"
        );
        let config = text(&s, "config/config.json");
        assert!(config.contains("./lib/webparts/{__componentNameCamel__}/{__componentNamePascal__}WebPart.js"), "{config}");
        assert!(config.contains("lib/webparts/{__componentNameCamel__}/loc/{locale}.js"), "{config}");
        // the css class and the strings module are not folder names, and have no slash after them
        assert!(text(&s, "src/webparts/{__componentNameCamel__}/{__componentNamePascal__}WebPart.module.scss").starts_with(".{__componentNameCamel__}WebPart {"));
        assert!(text(&s, "src/webparts/{__componentNameCamel__}/loc/{__componentNameCamel__}WebPartStrings.d.ts").contains("'{__componentNamePascal__}WebPartStrings'"));
    }

    #[test]
    fn a_rewrite_only_applies_to_the_templates_it_names() {
        let (t, mut s) = load(&[("src/{componentName.camel}WebPart/a.ts", b"x")]).unwrap();
        let overlay = Overlay::load(
            &Mem::new(&[("overlay.json", REWRITE.replace("\"demo-part\", ", "").replace("\"other\"", "\"webpart-react\"").as_bytes())]),
            "",
        )
        .unwrap();
        assert_eq!(s.apply_rewrites(&t, &overlay).unwrap(), 0);
        assert!(s.list_files(TEMPLATE_FOLDER).unwrap().contains(&"src/{__componentNameCamel__}WebPart/a.ts".to_string()));
    }

    #[test]
    fn a_rewrite_that_would_put_two_files_at_one_path_is_refused() {
        let (t, mut s) = load(&[("src/{componentName.camel}WebPart/a.ts", b"x"), ("src/{componentName.camel}/a.ts", b"y")]).unwrap();
        let overlay = Overlay::load(&Mem::new(&[("overlay.json", REWRITE.as_bytes())]), "").unwrap();
        let err = s.apply_rewrites(&t, &overlay).unwrap_err().to_string();
        assert!(err.contains("two files"), "{err}");
    }

    #[test]
    fn a_rewrite_needs_a_reason_and_a_safe_target() {
        let load = |rule: &str| Overlay::load(&Mem::new(&[("overlay.json", format!(r#"{{ "replace": {{}}, "rewrite": [ {rule} ] }}"#).as_bytes())]), "");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "y/", "why": "because" }"#).is_ok());
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "y/", "why": " " }"#).is_err(), "no reason");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "y/" }"#).is_err(), "no why at all");
        assert!(load(r#"{ "templates": [], "from": "x/", "to": "y/", "why": "w" }"#).is_err(), "no template");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "x/", "why": "w" }"#).is_err(), "changes nothing");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "../y/", "why": "w" }"#).is_err(), "leaves the template");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "/y/", "why": "w" }"#).is_err(), "absolute");
        assert!(load(r#"{ "templates": ["a"], "from": "x/", "to": "y/", "why": "w", "extra": 1 }"#).is_err(), "unknown key");
    }

    #[test]
    fn overlay_files_must_exist_and_unknown_keys_are_refused() {
        let missing = Mem::new(&[("overlay.json", br#"{ "replace": { "a.png": "nope.png" } }"#)]);
        assert!(Overlay::load(&missing, "").is_err());
        let typo = Mem::new(&[("overlay.json", br#"{ "replaces": {} }"#)]);
        assert!(Overlay::load(&typo, "").is_err());
        let escape = Mem::new(&[("overlay.json", br#"{ "replace": { "a.png": "../x.png" } }"#), ("x.png", b"x")]);
        assert!(Overlay::load(&escape, "").is_err());
        assert!(Overlay::load(&Mem::new(&[]), "").is_err());
    }

    #[test]
    fn the_node_range_of_the_template_goes_into_its_manifest() {
        let (t, _) = load(&[("package.json", br#"{ "name": "<%= libraryName %>", "engines": { "node": ">=22.14.0 <23.0.0" } }"#)]).unwrap();
        assert_eq!(t.node.as_deref(), Some(">=22.14.0 <23.0.0"));
        let m = t.manifest().unwrap();
        assert_eq!(m.requires.as_ref().and_then(|r| r.node.as_deref()), Some(">=22.14.0 <23.0.0"));
        assert!(matches!(m.node_check("v18.20.8").unwrap(), crate::node_range::NodeCheck::Mismatch { .. }));

        let (plain, _) = load(&[("package.json", b"{ \"name\": \"x\" }")]).unwrap();
        assert!(plain.manifest().unwrap().requires.is_none());
        let (none, _) = load(&[]).unwrap();
        assert!(none.node.is_none());
    }

    // ------------------------------------------------- display text of the official templates

    fn named(name: &str, category: &str, description: Option<&str>) -> UpstreamTemplate {
        serde_json::from_value(serde_json::json!({
            "name": name, "category": category, "description": description,
            "version": "0.0.1", "spfxVersion": "1.24.1"
        }))
        .unwrap()
    }

    #[test]
    fn a_title_is_made_from_the_name_of_an_official_template() {
        let title = |n: &str| named(n, "webpart", None).display().0;
        assert_eq!(title("webpart-react"), "Web part (React)");
        assert_eq!(title("webpart-noframework"), "Web part (no framework)");
        assert_eq!(title("webpart-minimal"), "Web part (minimal)");
        assert_eq!(title("extension-fieldcustomizer-react"), "Extension field customizer (React)");
        assert_eq!(title("extension-listviewcommandset"), "Extension list view command set");
        assert_eq!(title("ace-generic-primarytext-card"), "ACE generic primary text card");
        assert_eq!(title("library"), "Library");
        let (_, description, tags) = named("webpart-react", "webpart", Some("A React-based web part template for SPFx")).display();
        assert_eq!(description.as_deref(), Some("A React-based web part template for SPFx"));
        assert_eq!(tags, ["spfx", "webpart"]);
        assert!(named("library", "Some Category!", Some("  ")).display().1.is_none(), "a blank description is none");
        assert_eq!(named("library", "Some Category!", None).display().2, ["spfx", "some-category"]);
    }
}
