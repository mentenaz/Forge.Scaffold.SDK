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
            "packageManagers": { "default": "npm", "tested": ["npm"] }
        });
        Manifest::from_json(&value.to_string())
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
        let template: UpstreamTemplate = serde_json::from_str(&source.read_text(&json_path)?)
            .map_err(|e| bad(&json_path, format!("invalid template.json: {e}")))?;

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
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
                    text.into_bytes()
                }
                (_, Ok(text)) => text.into_bytes(),
                (_, Err(e)) => e.into_bytes(),
            };

            if files.insert(translated.clone(), bytes).is_some() {
                return Err(bad(&full, format!("two files would be written to '{translated}'")));
            }
        }
        Ok((template, UpstreamSource { files }))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
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
}
