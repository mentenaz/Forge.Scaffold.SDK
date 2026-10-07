//! Generator manifest: the questions to ask and how to expand the templates.
//!
//! The manifest is plain data (JSON). It contains no code, so a generator can
//! never run anything on the user's machine. The only "actions" it can ask for
//! are the fixed [`PostAction`]s, and those are returned to the host as
//! descriptors; the crate never executes them.
//!
//! ## Template layout
//!
//! All folder and file references are relative to the template directory (the
//! one that holds `manifest.json`). A `folder` is copied with its own name
//! stripped: `webpart/src/x.ts` in folder `webpart` ends up at `src/x.ts`.

use crate::error::{Result, ScaffoldError};
use crate::stage::normalize_rel;
use crate::tokens::is_valid_token_name;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Generator id, e.g. `spfx-webpart`.
    pub id: String,
    /// SPFx version these templates were made from, e.g. `1.23.2`.
    pub spfx_version: String,
    /// Questions the panel shows.
    pub prompts: Vec<Prompt>,
    /// Key of the `text` prompt whose answer is the name of the new folder.
    /// Required to run `plan()`.
    #[serde(default)]
    pub folder_from: Option<String>,
    /// File whose presence in the location (or a parent) means "you are inside
    /// an existing solution", e.g. `.yo-rc.json`. Produces a warning.
    #[serde(default)]
    pub nested_marker: Option<String>,
    /// Solution-level GUID tokens (one value for the whole run).
    #[serde(default)]
    pub guids: Vec<String>,
    /// Generated secrets (alphanumeric, always with an upper, a lower and a
    /// digit). One value per run, available as tokens.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Template folders copied once.
    #[serde(default)]
    pub include: Vec<Include>,
    /// Template folders that are expanded once per item of a list prompt.
    #[serde(default)]
    pub repeat: Vec<Repeat>,
    /// Text files assembled from ordered fragments (e.g. a compose file).
    #[serde(default)]
    pub parts: Vec<Part>,
    /// Edits to shared files after everything is rendered.
    #[serde(default)]
    pub merge: Vec<Merge>,
    /// Steps the host should run afterwards. Never executed by this crate.
    #[serde(default)]
    pub post: Vec<PostStep>,
    /// Which package managers the generator was tested with. Optional.
    #[serde(default)]
    pub package_managers: Option<PackageManagers>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
}

/// What the generator author tested. The panel offers every package manager it
/// finds on the PATH, and warns for those not in `tested`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManagers {
    pub default: PackageManager,
    pub tested: Vec<PackageManager>,
}

impl PackageManagers {
    /// Used when the manifest has no `packageManagers` field.
    pub fn fallback() -> Self {
        Self {
            default: PackageManager::Npm,
            tested: Vec::new(),
        }
    }

    pub fn is_tested(&self, pm: PackageManager) -> bool {
        self.tested.contains(&pm)
    }
}

/// A condition on the answers. Exactly one form per object:
///
/// * `{ "key": "db", "equals": "postgres" }`
/// * `{ "key": "db", "in": ["postgres", "mysql"] }`
/// * `{ "all": [ <condition>, <condition> ] }`
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// Key of a `choice` or `bool` prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Required answer: a string for `choice`, `true`/`false` for `bool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_json::Value>,
    /// The answer must be one of these.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<serde_json::Value>>,
    /// Every nested condition must hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<When>>,
}

impl When {
    /// Evaluates the condition against resolved answers. An unknown key never matches.
    pub fn matches(&self, answers: &BTreeMap<String, serde_json::Value>) -> bool {
        if let Some(all) = &self.all {
            return all.iter().all(|w| w.matches(answers));
        }
        let Some(key) = &self.key else { return false };
        let Some(actual) = answers.get(key) else { return false };
        if let Some(eq) = &self.equals {
            return actual == eq;
        }
        if let Some(options) = &self.any_of {
            return options.iter().any(|o| o == actual);
        }
        false
    }
}

/// Evaluates an optional condition: no condition means "always".
pub fn when_holds(when: &Option<When>, answers: &BTreeMap<String, serde_json::Value>) -> bool {
    when.as_ref().is_none_or(|w| w.matches(answers))
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    /// Answer key. For `text` prompts this is also the token name.
    pub key: String,
    #[serde(rename = "type")]
    pub kind: PromptKind,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Allowed values. Required for `choice`, not allowed for other types.
    #[serde(default)]
    pub options: Vec<String>,
    /// Text prompts only: the panel masks the input, and the value is never
    /// logged or included in warnings. It is a plain token (no derived casings).
    #[serde(default, skip_serializing_if = "is_false")]
    pub sensitive: bool,
    /// Built-in name rules to enforce on the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<NameRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameRule {
    /// `text` prompt, checked with [`crate::validate_solution_name`].
    SolutionName,
    /// `list` prompt, checked with [`crate::validate_webpart_names`].
    WebpartNames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptKind {
    Text,
    Bool,
    List,
    /// Pick exactly one of `options`.
    Choice,
}

/// A template folder copied once.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Include {
    /// Template folder, relative to the template root. Its name is stripped.
    pub folder: String,
    /// Optional destination prefix inside the new solution.
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub when: Option<When>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Repeat {
    /// Key of a `list` prompt to iterate over.
    pub over: String,
    /// Token name for the current item, e.g. `webpartNaam`.
    #[serde(rename = "as")]
    pub item: String,
    /// Template folder (relative to the template root) expanded per item.
    pub folder: String,
    /// GUID tokens generated fresh for every item.
    #[serde(default)]
    pub guids: Vec<String>,
    /// Only expand this folder when the condition holds (e.g. React vs none).
    #[serde(default)]
    pub when: Option<When>,
}

/// One fragment of an assembled text file. Fragments for the same `target` are
/// concatenated in manifest order; fragments whose `when` fails are skipped.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Output file (may contain tokens).
    pub target: String,
    /// Template file with the fragment text.
    pub source: String,
    #[serde(default)]
    pub when: Option<When>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Merge {
    /// Staged file to edit. May contain tokens.
    pub file: String,
    pub strategy: MergeStrategy,
    /// Dotted JSON path. May contain tokens, e.g. `bundles.{__webpartNaamKebab__}`.
    pub path: String,
    /// JSON value to write/append. String values may contain tokens.
    /// Exactly one of `value` and `valueFile`.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Template file with a JSON fragment. It is rendered as text first and
    /// parsed afterwards, so `"port": {__dbPort__}` stays a number.
    #[serde(default)]
    pub value_file: Option<String>,
    /// Apply once per item of this list prompt (must match a `repeat.over`).
    #[serde(default)]
    pub per: Option<String>,
    /// Only apply this merge when the condition holds.
    #[serde(default)]
    pub when: Option<When>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStrategy {
    /// Append `value` to the array at `path`.
    JsonAppend,
    /// Set `path` to `value`, creating intermediate objects.
    JsonSet,
}

/// The fixed set of things a generator may ask the host to do afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PostAction {
    /// Install dependencies with the chosen package manager.
    Install,
    /// `docker compose up -d` in the solution folder.
    ComposeUp,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PostStep {
    pub action: PostAction,
    /// Folder inside the new solution to run in (default: the solution root).
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub when: Option<When>,
}

impl Manifest {
    /// Parses and validates a manifest.
    pub fn from_json(json: &str) -> Result<Manifest> {
        let manifest: Manifest =
            serde_json::from_str(json).map_err(|e| ScaffoldError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks that the manifest is internally consistent. All problems are
    /// reported together.
    pub fn validate(&self) -> Result<()> {
        let mut issues: Vec<String> = Vec::new();

        if self.id.trim().is_empty() {
            issues.push("'id' must not be empty".into());
        }
        if !is_semver_triplet(&self.spfx_version) {
            issues.push(format!(
                "'spfxVersion' must look like 1.23.2, got '{}'",
                self.spfx_version
            ));
        }

        // Prompt keys
        let mut keys: HashSet<&str> = HashSet::new();
        for p in &self.prompts {
            if !is_valid_token_name(&p.key) {
                issues.push(format!("prompt key '{}' must be ASCII letters and digits", p.key));
            }
            if !keys.insert(p.key.as_str()) {
                issues.push(format!("prompt key '{}' is defined twice", p.key));
            }
        }

        // Choice prompts, defaults and options; sensitive and rule placement
        for p in &self.prompts {
            match p.kind {
                PromptKind::Choice => {
                    if p.options.is_empty() {
                        issues.push(format!("choice prompt '{}' needs at least one option", p.key));
                    }
                    let mut seen: HashSet<&str> = HashSet::new();
                    for o in &p.options {
                        if o.trim().is_empty() {
                            issues.push(format!("choice prompt '{}' has an empty option", p.key));
                        } else if !seen.insert(o.as_str()) {
                            issues.push(format!("choice prompt '{}' lists option '{o}' twice", p.key));
                        }
                    }
                    if let Some(d) = &p.default {
                        match d.as_str() {
                            Some(v) if p.options.iter().any(|o| o == v) => {}
                            _ => issues.push(format!(
                                "choice prompt '{}': default must be one of its options",
                                p.key
                            )),
                        }
                    }
                }
                _ => {
                    if !p.options.is_empty() {
                        issues.push(format!(
                            "prompt '{}': 'options' is only allowed on choice prompts",
                            p.key
                        ));
                    }
                }
            }
            if p.sensitive && p.kind != PromptKind::Text {
                issues.push(format!("prompt '{}': 'sensitive' is only allowed on text prompts", p.key));
            }
            match (p.rule, p.kind) {
                (Some(NameRule::SolutionName), PromptKind::Text) => {}
                (Some(NameRule::WebpartNames), PromptKind::List) => {}
                (Some(NameRule::SolutionName), _) => issues.push(format!(
                    "prompt '{}': rule 'solution-name' needs a text prompt",
                    p.key
                )),
                (Some(NameRule::WebpartNames), _) => issues.push(format!(
                    "prompt '{}': rule 'webpart-names' needs a list prompt",
                    p.key
                )),
                (None, _) => {}
            }
        }

        // Token names must be globally unique, including derived forms.
        let mut tokens: HashSet<String> = HashSet::new();
        let mut claim = |name: &str, with_forms: bool, issues: &mut Vec<String>| {
            if !is_valid_token_name(name) {
                issues.push(format!("token name '{name}' must be ASCII letters and digits"));
                return;
            }
            let mut all = vec![name.to_string()];
            if with_forms {
                for suffix in ["Pascal", "Camel", "Kebab"] {
                    all.push(format!("{name}{suffix}"));
                }
            }
            for t in all {
                if !tokens.insert(t.clone()) {
                    issues.push(format!(
                        "token '{t}' is defined more than once (or collides with a derived form of another token)"
                    ));
                }
            }
        };

        for p in &self.prompts {
            match p.kind {
                // Sensitive text is a plain token: no derived casings.
                PromptKind::Text => claim(&p.key, !p.sensitive, &mut issues),
                PromptKind::Choice => claim(&p.key, false, &mut issues),
                _ => {}
            }
        }
        for g in &self.guids {
            claim(g, false, &mut issues);
        }
        for s in &self.secrets {
            claim(s, false, &mut issues);
        }
        for r in &self.repeat {
            claim(&r.item, true, &mut issues);
            for g in &r.guids {
                claim(g, false, &mut issues);
            }
        }

        // folderFrom
        if let Some(key) = &self.folder_from {
            match self.prompts.iter().find(|p| &p.key == key) {
                Some(p) if p.kind == PromptKind::Text && !p.sensitive => {}
                _ => issues.push(format!("folderFrom '{key}' must be a non-sensitive text prompt")),
            }
        }
        if let Some(marker) = &self.nested_marker {
            if let Err(e) = normalize_rel(marker) {
                issues.push(format!("nestedMarker: {e}"));
            }
        }

        // Includes
        for inc in &self.include {
            if let Err(e) = normalize_rel(&inc.folder) {
                issues.push(format!("include folder: {e}"));
            }
            if let Some(to) = &inc.to {
                if let Err(e) = normalize_rel(to) {
                    issues.push(format!("include to: {e}"));
                }
            }
        }

        // Repeats
        for r in &self.repeat {
            match self.prompts.iter().find(|p| p.key == r.over) {
                None => issues.push(format!("repeat over '{}': no such prompt", r.over)),
                Some(p) if p.kind != PromptKind::List => issues.push(format!(
                    "repeat over '{}': the prompt must be of type 'list'",
                    r.over
                )),
                Some(_) => {}
            }
            if let Err(e) = normalize_rel(&r.folder) {
                issues.push(format!("repeat folder: {e}"));
            }
        }

        // Parts
        for p in &self.parts {
            if let Err(e) = normalize_rel(&p.target) {
                issues.push(format!("part target: {e}"));
            }
            if let Err(e) = normalize_rel(&p.source) {
                issues.push(format!("part source: {e}"));
            }
        }

        // Merges
        for m in &self.merge {
            if let Err(e) = normalize_rel(&m.file) {
                issues.push(format!("merge file: {e}"));
            }
            if m.path.trim().is_empty() {
                issues.push(format!("merge on '{}': 'path' must not be empty", m.file));
            }
            match (&m.value, &m.value_file) {
                (Some(_), None) => {}
                (None, Some(f)) => {
                    if let Err(e) = normalize_rel(f) {
                        issues.push(format!("merge valueFile: {e}"));
                    }
                }
                _ => issues.push(format!(
                    "merge on '{}': give exactly one of 'value' and 'valueFile'",
                    m.file
                )),
            }
            if let Some(per) = &m.per {
                if !self.repeat.iter().any(|r| &r.over == per) {
                    issues.push(format!(
                        "merge on '{}': per '{per}' does not match any repeat",
                        m.file
                    ));
                }
            }
        }

        // Post steps
        for s in &self.post {
            if let Some(cwd) = &s.cwd {
                if let Err(e) = normalize_rel(cwd) {
                    issues.push(format!("post cwd: {e}"));
                }
            }
        }

        // Conditions
        for (i, inc) in self.include.iter().enumerate() {
            self.check_when(&format!("include #{i} '{}'", inc.folder), &inc.when, &mut issues);
        }
        for r in &self.repeat {
            self.check_when(&format!("repeat '{}'", r.folder), &r.when, &mut issues);
        }
        for p in &self.parts {
            self.check_when(&format!("part '{}'", p.source), &p.when, &mut issues);
        }
        for m in &self.merge {
            self.check_when(&format!("merge '{}'", m.file), &m.when, &mut issues);
        }
        for s in &self.post {
            self.check_when(&format!("post {:?}", s.action), &s.when, &mut issues);
        }

        // Package managers
        if let Some(pm) = &self.package_managers {
            let mut seen = HashSet::new();
            for t in &pm.tested {
                if !seen.insert(*t) {
                    issues.push(format!("packageManagers.tested lists {t:?} twice"));
                }
            }
            if !pm.tested.contains(&pm.default) {
                issues.push("packageManagers.default must also be listed in 'tested'".into());
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ScaffoldError::Manifest(issues.join("; ")))
        }
    }

    fn check_when(&self, what: &str, when: &Option<When>, issues: &mut Vec<String>) {
        if let Some(w) = when {
            self.check_when_expr(what, w, issues);
        }
    }

    fn check_when_expr(&self, what: &str, w: &When, issues: &mut Vec<String>) {
        if let Some(all) = &w.all {
            if w.key.is_some() || w.equals.is_some() || w.any_of.is_some() {
                issues.push(format!("{what}: 'all' cannot be combined with 'key', 'equals' or 'in'"));
            }
            if all.is_empty() {
                issues.push(format!("{what}: 'all' must list at least one condition"));
            }
            for inner in all {
                self.check_when_expr(what, inner, issues);
            }
            return;
        }

        let Some(key) = &w.key else {
            issues.push(format!("{what}: 'when' needs a 'key' (or 'all')"));
            return;
        };
        let values: Vec<&serde_json::Value> = match (&w.equals, &w.any_of) {
            (Some(v), None) => vec![v],
            (None, Some(list)) if !list.is_empty() => list.iter().collect(),
            (None, Some(_)) => {
                issues.push(format!("{what}: 'in' must list at least one value"));
                return;
            }
            _ => {
                issues.push(format!("{what}: 'when' needs exactly one of 'equals' and 'in'"));
                return;
            }
        };

        match self.prompts.iter().find(|p| &p.key == key) {
            None => issues.push(format!("{what}: 'when' refers to unknown prompt '{key}'")),
            Some(p) => match p.kind {
                PromptKind::Choice => {
                    for v in values {
                        match v.as_str() {
                            Some(s) if p.options.iter().any(|o| o == s) => {}
                            _ => issues.push(format!(
                                "{what}: 'when' value for '{key}' must be one of its options"
                            )),
                        }
                    }
                }
                PromptKind::Bool => {
                    for v in values {
                        if !v.is_boolean() {
                            issues.push(format!("{what}: 'when' value for '{key}' must be true or false"));
                        }
                    }
                }
                _ => issues.push(format!(
                    "{what}: 'when' can only test choice or bool prompts, '{key}' is not one"
                )),
            },
        }
    }
}

fn is_semver_triplet(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}


#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
      "id": "spfx-webpart",
      "spfxVersion": "1.23.2",
      "prompts": [
        { "key": "solutionName", "type": "text", "label": "Solution name" },
        { "key": "webparts", "type": "list", "label": "Web parts" },
        { "key": "install", "type": "bool", "default": false }
      ],
      "guids": ["solutionId"],
      "repeat": [
        { "over": "webparts", "as": "webpartNaam", "folder": "webpart", "guids": ["webpartId"] }
      ],
      "merge": [
        {
          "file": "config/config.json",
          "strategy": "json-set",
          "path": "bundles.{__webpartNaamKebab__}",
          "per": "webparts",
          "value": { "components": [] }
        }
      ]
    }"#;

    fn mutate(from: &str, to: &str) -> String {
        assert!(GOOD.contains(from), "test bug: {from} not in GOOD");
        GOOD.replacen(from, to, 1)
    }

    fn err_of(json: &str) -> String {
        Manifest::from_json(json).unwrap_err().to_string()
    }

    #[test]
    fn parses_a_good_manifest() {
        let m = Manifest::from_json(GOOD).unwrap();
        assert_eq!(m.id, "spfx-webpart");
        assert_eq!(m.prompts.len(), 3);
        assert_eq!(m.prompts[1].kind, PromptKind::List);
        assert_eq!(m.repeat[0].item, "webpartNaam");
        assert_eq!(m.merge[0].strategy, MergeStrategy::JsonSet);
        assert_eq!(m.merge[0].per.as_deref(), Some("webparts"));
    }

    #[test]
    fn round_trips_through_json() {
        let m = Manifest::from_json(GOOD).unwrap();
        let again = Manifest::from_json(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, again);
    }

    #[test]
    fn unknown_fields_are_rejected_to_catch_typos() {
        assert!(err_of(&mutate("\"spfxVersion\"", "\"spfxVersoin\"")).contains("Invalid manifest"));
        assert!(err_of(&mutate("\"folder\"", "\"folders\"")).contains("Invalid manifest"));
    }

    #[test]
    fn version_must_be_a_triplet() {
        assert!(err_of(&mutate("1.23.2", "1.23")).contains("spfxVersion"));
    }

    #[test]
    fn repeat_must_point_at_a_list_prompt() {
        assert!(err_of(&mutate("\"over\": \"webparts\"", "\"over\": \"nope\"")).contains("no such prompt"));
        assert!(err_of(&mutate("\"over\": \"webparts\"", "\"over\": \"install\"")).contains("type 'list'"));
    }

    #[test]
    fn token_names_must_be_unique_including_derived_forms() {
        // A text prompt called "webpartNaamPascal" collides with the derived form of the repeat item.
        let json = mutate(
            "{ \"key\": \"install\", \"type\": \"bool\", \"default\": false }",
            "{ \"key\": \"install\", \"type\": \"bool\" }, { \"key\": \"webpartNaamPascal\", \"type\": \"text\" }",
        );
        assert!(err_of(&json).contains("webpartNaamPascal"));

        let dup_guid = mutate("\"guids\": [\"webpartId\"]", "\"guids\": [\"solutionId\"]");
        assert!(err_of(&dup_guid).contains("solutionId"));
    }

    #[test]
    fn duplicate_prompt_keys_are_rejected() {
        let json = mutate("\"key\": \"install\"", "\"key\": \"webparts\"");
        assert!(err_of(&json).contains("defined twice"));
    }

    #[test]
    fn unsafe_folders_and_files_are_rejected() {
        assert!(err_of(&mutate("\"folder\": \"webpart\"", "\"folder\": \"../webpart\"")).contains("repeat folder"));
        assert!(err_of(&mutate("config/config.json", "/etc/config.json")).contains("merge file"));
    }

    #[test]
    fn package_managers_are_optional_and_validated() {
        // absent: fine, and the fallback is npm with nothing marked tested
        let m = Manifest::from_json(GOOD).unwrap();
        assert!(m.package_managers.is_none());
        assert_eq!(PackageManagers::fallback().default, PackageManager::Npm);

        let with = mutate(
            "\"guids\": [\"solutionId\"],",
            "\"guids\": [\"solutionId\"], \"packageManagers\": { \"default\": \"npm\", \"tested\": [\"npm\", \"pnpm\"] },",
        );
        let m = Manifest::from_json(&with).unwrap();
        let pm = m.package_managers.unwrap();
        assert!(pm.is_tested(PackageManager::Pnpm));
        assert!(!pm.is_tested(PackageManager::Yarn));

        let bad_default = with.replace("\"default\": \"npm\"", "\"default\": \"yarn\"");
        assert!(err_of(&bad_default).contains("must also be listed"));

        let dup = with.replace("[\"npm\", \"pnpm\"]", "[\"npm\", \"npm\"]");
        assert!(err_of(&dup).contains("twice"));

        let unknown = with.replace("\"pnpm\"", "\"bun\"");
        assert!(err_of(&unknown).contains("Invalid manifest"));
    }

    const CHOICE: &str = r#"{
      "id": "spfx-webpart",
      "spfxVersion": "1.23.2",
      "prompts": [
        { "key": "webparts", "type": "list" },
        { "key": "framework", "type": "choice", "options": ["react", "none"], "default": "react" },
        { "key": "install", "type": "bool" }
      ],
      "repeat": [
        { "over": "webparts", "as": "webpartNaam", "folder": "webpart-react",
          "when": { "key": "framework", "equals": "react" } },
        { "over": "webparts", "as": "webpartNaamNone", "folder": "webpart-none",
          "when": { "key": "framework", "equals": "none" } }
      ]
    }"#;

    #[test]
    fn choice_prompts_and_conditional_repeats_parse() {
        let m = Manifest::from_json(CHOICE).unwrap();
        assert_eq!(m.prompts[1].kind, PromptKind::Choice);
        assert_eq!(m.prompts[1].options, vec!["react", "none"]);
        assert_eq!(m.repeat[0].when.as_ref().unwrap().key.as_deref(), Some("framework"));
    }

    #[test]
    fn choice_rules_are_enforced() {
        let no_options = CHOICE.replace("\"options\": [\"react\", \"none\"], ", "");
        assert!(err_of(&no_options).contains("at least one option"));

        let dup = CHOICE.replace("[\"react\", \"none\"]", "[\"react\", \"react\"]");
        assert!(err_of(&dup).contains("twice"));

        let bad_default = CHOICE.replace("\"default\": \"react\"", "\"default\": \"vue\"");
        assert!(err_of(&bad_default).contains("default must be one of"));

        let options_on_text = CHOICE.replace(
            "{ \"key\": \"install\", \"type\": \"bool\" }",
            "{ \"key\": \"install\", \"type\": \"bool\", \"options\": [\"a\"] }",
        );
        assert!(err_of(&options_on_text).contains("only allowed on choice"));
    }

    #[test]
    fn when_conditions_are_checked_against_the_prompts() {
        let unknown_key = CHOICE.replacen("\"key\": \"framework\", \"equals\"", "\"key\": \"nope\", \"equals\"", 1);
        assert!(err_of(&unknown_key).contains("unknown prompt"));

        let bad_value = CHOICE.replacen("\"equals\": \"react\"", "\"equals\": \"vue\"", 1);
        assert!(err_of(&bad_value).contains("one of its options"));

        let on_list = CHOICE.replacen("\"key\": \"framework\", \"equals\"", "\"key\": \"webparts\", \"equals\"", 1);
        assert!(err_of(&on_list).contains("choice or bool"));

        let bool_ok = CHOICE.replacen(
            "\"when\": { \"key\": \"framework\", \"equals\": \"react\" }",
            "\"when\": { \"key\": \"install\", \"equals\": true }",
            1,
        );
        assert!(Manifest::from_json(&bool_ok).is_ok());

        let bool_bad = bool_ok.replacen("\"equals\": true", "\"equals\": \"yes\"", 1);
        assert!(err_of(&bool_bad).contains("true or false"));
    }

    #[test]
    fn merge_per_must_match_a_repeat() {
        let json = mutate("\"per\": \"webparts\"", "\"per\": \"other\"");
        assert!(err_of(&json).contains("does not match any repeat"));
    }
}
