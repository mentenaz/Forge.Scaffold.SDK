//! Checking a template before anyone uses it: does the manifest load, does the template render, and is anything
//! in it unused or suspicious.
//!
//! [`check_template`] loads `manifest.json` from a [`TemplateSource`] (a generator folder, a pack, a converted upstream
//! template), validates it, and then dry-runs the plan with sample answers. It runs the template once with the defaults
//! and once more for every other value of every yes/no and choice prompt, so a branch behind a `when` is rendered too.
//! Nothing is written and no folder on disk is looked at (the plan is dry: see `plan_dry`).
//!
//! What it reports, in two kinds:
//!
//! * **problems**: the manifest does not load, a variant of the template does not render (with the error, which names the
//!   file), or the template is over a limit. A template with a problem should not be used.
//! * **notes**: things that are probably mistakes but break nothing: a prompt that nothing uses, a file in the template
//!   that no `include`, `repeat`, `parts` or `merge` refers to. And, per variant, the plan's own warnings (install-time
//!   scripts, URL dependencies, files that tools run by themselves), which are about what the template does and not about
//!   whether it works.
//!
//! This is the structural check. It does not read the code a template writes: that is for the separate analysis engine.

use crate::add::collect_when;
use crate::manifest::{Manifest, PromptKind, NameRule, PostAction};
use crate::plan::{plan_dry, Answers, PlanOptions};
use crate::secrets::SequentialSecrets;
use crate::source::TemplateSource;
use crate::stage::Limits;
use crate::tokens::SequentialGuids;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

/// How to check.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Answers to use instead of the sample ones (a prompt that is not named here gets its default, or a sample value).
    pub answers: Answers,
    /// The limits the plan is held to.
    pub limits: Limits,
    /// At most this many variants are rendered (the defaults, then one per other value of each yes/no and choice prompt).
    pub max_variants: usize,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { answers: Answers::new(), limits: Limits::default(), max_variants: 32 }
    }
}

/// Something wrong with the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Where: `manifest`, or the variant it happened in (`defaults`, `install = true`).
    pub at: String,
    pub message: String,
}

/// One rendering of the template with one set of answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// What differs from the defaults: `defaults`, `install = true`, `framework = vue`.
    pub label: String,
    /// Files and bytes the plan would write. Zero when it did not render.
    pub files: usize,
    pub bytes: u64,
    /// The plan's warnings, as text.
    pub warnings: Vec<String>,
    /// The steps the plan hands the host, as text (`install (npm)`, `composeUp`).
    pub post: Vec<String>,
    /// Why it did not render, if it did not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TemplateReport {
    /// The generator's id and version, when the manifest loaded.
    pub id: Option<String>,
    pub version: Option<String>,
    pub requires_node: Option<String>,
    pub problems: Vec<Problem>,
    pub notes: Vec<String>,
    pub variants: Vec<Variant>,
}

impl TemplateReport {
    /// No problems. Notes and warnings do not count.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The report as JSON, for a host that is not written in Rust.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "id": self.id,
            "version": self.version,
            "requiresNode": self.requires_node,
            "problems": self.problems.iter().map(|p| json!({ "at": p.at, "message": p.message })).collect::<Vec<_>>(),
            "notes": self.notes,
            "variants": self.variants.iter().map(|v| json!({
                "label": v.label, "files": v.files, "bytes": v.bytes,
                "warnings": v.warnings, "post": v.post, "error": v.error,
            })).collect::<Vec<_>>(),
        })
    }
}

impl fmt::Display for TemplateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match (&self.id, &self.version) {
            (Some(id), Some(v)) => format!("{id} {v}"),
            _ => "template".to_string(),
        };
        if self.ok() {
            writeln!(f, "{name}: no problems")?;
        } else {
            writeln!(f, "{name}: {} problem(s)", self.problems.len())?;
        }
        for p in &self.problems {
            writeln!(f, "  problem [{}]: {}", p.at, p.message)?;
        }
        for v in &self.variants {
            match &v.error {
                Some(_) => writeln!(f, "  {}: did not render", v.label)?,
                None => writeln!(f, "  {}: {} files, {} bytes, {} warning(s)", v.label, v.files, v.bytes, v.warnings.len())?,
            }
            for w in &v.warnings {
                writeln!(f, "      warning: {w}")?;
            }
        }
        for n in &self.notes {
            writeln!(f, "  note: {n}")?;
        }
        Ok(())
    }
}

/// Checks the template in `source`, whose `manifest.json` is at its root.
pub fn check_template(source: &dyn TemplateSource, options: &CheckOptions) -> TemplateReport {
    match source.read_text("manifest.json") {
        Ok(text) => check_with_manifest(&text, source, options),
        Err(e) => TemplateReport {
            problems: vec![Problem { at: "manifest".into(), message: format!("manifest.json cannot be read: {e}") }],
            ..TemplateReport::default()
        },
    }
}

/// Checks a template whose manifest text is given separately (a converted upstream template has its manifest made, not stored).
pub fn check_with_manifest(manifest_text: &str, source: &dyn TemplateSource, options: &CheckOptions) -> TemplateReport {
    let mut report = TemplateReport::default();

    let manifest = match Manifest::from_json(manifest_text) {
        Ok(m) => m,
        Err(e) => {
            // all problems come back together, one per line
            for line in e.to_string().lines().map(str::trim).filter(|l| !l.is_empty()) {
                report.problems.push(Problem { at: "manifest".into(), message: line.to_string() });
            }
            return report;
        }
    };
    report.id = Some(manifest.id.clone());
    report.version = Some(manifest.version.clone());
    report.requires_node = manifest.requires.as_ref().and_then(|r| r.node.clone());

    // ---- render it: the defaults, then each other value of each yes/no and choice prompt
    let base = sample_answers(&manifest, &options.answers);
    let mut variants: Vec<(String, Answers)> = vec![("defaults".to_string(), base.clone())];
    for p in &manifest.prompts {
        let others: Vec<Value> = match p.kind {
            PromptKind::Bool => vec![json!(!base.get(&p.key).and_then(Value::as_bool).unwrap_or(false))],
            PromptKind::Choice => p.options.iter().filter(|o| base.get(&p.key).and_then(Value::as_str) != Some(o)).map(|o| json!(o)).collect(),
            _ => Vec::new(),
        };
        if options.answers.contains_key(&p.key) {
            continue; // the caller fixed this one
        }
        for value in others {
            let mut answers = base.clone();
            answers.insert(p.key.clone(), value.clone());
            let shown = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
            variants.push((format!("{} = {shown}", p.key), answers));
        }
    }
    if variants.len() > options.max_variants {
        report.notes.push(format!(
            "{} variants of the answers exist; only the first {} were rendered",
            variants.len(),
            options.max_variants
        ));
        variants.truncate(options.max_variants);
    }

    for (label, answers) in variants {
        let plan_options = PlanOptions {
            location: PathBuf::from("forge-check"),
            package_manager: None,
            guids: Box::new(SequentialGuids::new()),
            secrets: Box::new(SequentialSecrets::new()),
            check_long_paths: false,
            limits: options.limits,
        };
        match plan_dry(&manifest, source, &answers, plan_options) {
            Ok(plan) => report.variants.push(Variant {
                label,
                files: plan.files.len(),
                bytes: plan.files.total_bytes(),
                warnings: plan.warnings.iter().map(|w| w.to_string()).collect(),
                post: plan
                    .post
                    .iter()
                    .map(|s| match (s.action, s.package_manager) {
                        (PostAction::Install, Some(pm)) => format!("install ({pm:?})").to_lowercase(),
                        (PostAction::Install, None) => "install".to_string(),
                        (PostAction::ComposeUp, _) => "composeUp".to_string(),
                    })
                    .collect(),
                error: None,
            }),
            Err(e) => {
                let message = match &e {
                    crate::error::ScaffoldError::Validation(issues) => {
                        issues.iter().map(|i| format!("{}: {}", i.field, i.message)).collect::<Vec<_>>().join("; ")
                    }
                    other => other.to_string(),
                };
                report.problems.push(Problem { at: label.clone(), message: message.clone() });
                report.variants.push(Variant { label, files: 0, bytes: 0, warnings: Vec::new(), post: Vec::new(), error: Some(message) });
            }
        }
    }

    unused_things(&manifest, manifest_text, source, &mut report.notes);
    // TODO(forge-analysis): the checks above are structural (does it load, does it render, is anything unused). Once the
    // analysis engine is a dependency, each rendered variant could also be analysed for what its code does. It reads a
    // folder on disk and this check writes nothing, so that needs either a scratch folder per variant or an in-memory
    // input in the analysis engine. Decide which when it is wired; not wired until it is finished, tested and confirmed.
    report
}

/// A value for every prompt: what the caller gave, else the default, else something plausible that passes the prompt's rule.
fn sample_answers(manifest: &Manifest, given: &Answers) -> Answers {
    let mut out = Answers::new();
    for p in &manifest.prompts {
        let value = if let Some(v) = given.get(&p.key) {
            v.clone()
        } else if let Some(d) = &p.default {
            d.clone()
        } else {
            match p.kind {
                PromptKind::Bool => json!(false),
                PromptKind::List => json!(["Sample"]),
                PromptKind::Choice => json!(p.options.first().cloned().unwrap_or_default()),
                PromptKind::Text if p.sensitive => json!("sample-secret"),
                PromptKind::Text => match p.rule {
                    Some(NameRule::SolutionName) | Some(NameRule::PackageName) => json!("sample-project"),
                    _ => json!("Sample"),
                },
            }
        };
        out.insert(p.key.clone(), value);
    }
    out
}

/// Every `{__name__}` in `text`.
fn tokens_in(text: &str, out: &mut HashSet<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("{__") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("__}") else { break };
        let name = &after[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.insert(name.to_string());
        }
        rest = &after[end + 3..];
    }
}

/// Notes about a prompt nothing uses and files nothing refers to.
fn unused_things(manifest: &Manifest, manifest_text: &str, source: &dyn TemplateSource, notes: &mut Vec<String>) {
    let Ok(files) = source.list_files("") else { return };

    // the folders and files the manifest names
    let mut used_dirs: Vec<String> = Vec::new();
    let mut used_files: Vec<String> = vec!["manifest.json".to_string()];
    used_dirs.extend(manifest.include.iter().map(|i| i.folder.trim_matches('/').to_string()));
    used_dirs.extend(manifest.repeat.iter().map(|r| r.folder.trim_matches('/').to_string()));
    used_files.extend(manifest.parts.iter().map(|p| p.source.clone()));
    used_files.extend(manifest.merge.iter().filter_map(|m| m.value_file.clone()));
    let is_used = |path: &str| {
        used_files.iter().any(|f| f == path)
            || used_dirs.iter().any(|d| path.starts_with(&format!("{d}/")))
            || matches!(path, "NOTICE.txt" | "LICENSE" | "LICENSE.txt" | "README.md")
    };
    let orphans: Vec<&String> = files.iter().filter(|f| !is_used(f)).collect();
    if !orphans.is_empty() {
        let shown: Vec<&str> = orphans.iter().take(5).map(|s| s.as_str()).collect();
        notes.push(format!(
            "{} file(s) in the template are not used by the manifest: {}{}",
            orphans.len(),
            shown.join(", "),
            if orphans.len() > shown.len() { ", ..." } else { "" }
        ));
    }

    // every token that appears anywhere: in paths, in text, and in the manifest itself
    let mut tokens: HashSet<String> = HashSet::new();
    tokens_in(manifest_text, &mut tokens);
    for path in &files {
        tokens_in(path, &mut tokens);
        if let Ok(text) = source.read_text(path) {
            tokens_in(&text, &mut tokens);
        }
    }
    // and every prompt the manifest itself refers to
    let mut referenced: HashSet<String> = HashSet::new();
    referenced.extend(manifest.folder_from.clone());
    for i in &manifest.include {
        collect_when(&i.when, &mut referenced);
    }
    for r in &manifest.repeat {
        referenced.insert(r.over.clone());
        collect_when(&r.when, &mut referenced);
    }
    for p in &manifest.parts {
        collect_when(&p.when, &mut referenced);
    }
    for m in &manifest.merge {
        referenced.extend(m.per.clone());
        collect_when(&m.when, &mut referenced);
    }
    for s in &manifest.post {
        collect_when(&s.when, &mut referenced);
    }
    for p in &manifest.prompts {
        let by_token = ["", "Pascal", "Camel", "Kebab", "AllCaps"].iter().any(|suffix| tokens.contains(&format!("{}{suffix}", p.key)));
        if !by_token && !referenced.contains(&p.key) {
            notes.push(format!("the prompt '{}' is not used by anything in the template", p.key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::DirSource;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-check-{}", uuid::Uuid::new_v4()));
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

    const GOOD: &str = r#"{
      "id": "demo", "version": "1.2.3", "folderFrom": "projectName",
      "requires": { "node": ">=20.0.0" },
      "prompts": [
        { "key": "projectName", "type": "text", "rule": "package-name" },
        { "key": "framework", "type": "choice", "options": ["react", "vue"], "default": "react" },
        { "key": "install", "type": "bool", "default": false }
      ],
      "include": [
        { "folder": "base" },
        { "folder": "react", "when": { "key": "framework", "equals": "react" } },
        { "folder": "vue", "when": { "key": "framework", "equals": "vue" } }
      ],
      "post": [ { "action": "install", "when": { "key": "install", "equals": true } } ]
    }"#;

    fn good() -> Tmp {
        let t = Tmp::new();
        t.write("manifest.json", GOOD);
        t.write("base/package.json", "{ \"name\": \"{__projectName__}\" }\n");
        t.write("react/src/App.tsx", "// react {__projectNamePascal__}\n");
        t.write("vue/src/App.vue", "<!-- vue -->\n");
        t
    }

    fn check(t: &Tmp) -> TemplateReport {
        check_template(&DirSource::new(&t.0), &CheckOptions::default())
    }

    #[test]
    fn a_good_template_has_no_problems_and_every_branch_is_rendered() {
        let r = check(&good());
        assert!(r.ok(), "{r}");
        assert_eq!((r.id.as_deref(), r.version.as_deref(), r.requires_node.as_deref()), (Some("demo"), Some("1.2.3"), Some(">=20.0.0")));
        let labels: Vec<&str> = r.variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, ["defaults", "framework = vue", "install = true"]);
        // react: package.json + App.tsx; vue: package.json + App.vue
        assert_eq!(r.variants[0].files, 2);
        assert_eq!(r.variants[1].files, 2);
        assert_eq!(r.variants[2].post, ["install (npm)"]);
        assert!(r.variants[0].post.is_empty());
        assert!(r.notes.is_empty(), "{:?}", r.notes);
        assert!(r.to_json()["ok"].as_bool().unwrap());
    }

    #[test]
    fn a_missing_or_broken_manifest_reports_every_problem_it_has() {
        let t = Tmp::new();
        let r = check(&t);
        assert!(!r.ok() && r.problems[0].message.contains("manifest.json cannot be read"), "{r}");

        t.write("manifest.json", r#"{ "id": "x", "version": "nope", "prompts": [], "unknownField": 1 }"#);
        let r = check(&t);
        assert!(!r.ok());
        assert!(r.problems.iter().all(|p| p.at == "manifest"), "{r}");
        assert!(r.variants.is_empty(), "nothing is rendered from a manifest that does not load");
    }

    #[test]
    fn a_problem_that_only_shows_in_one_branch_is_found_and_says_where() {
        let t = good();
        fs::remove_dir_all(t.0.join("vue")).unwrap(); // the vue branch now points at a folder that is gone
        let r = check(&t);
        assert!(!r.ok(), "{r}");
        assert_eq!(r.problems.len(), 1, "{r}");
        assert_eq!(r.problems[0].at, "framework = vue");
        assert!(r.variants[0].error.is_none() && r.variants[1].error.is_some());
        assert_eq!(r.variants[1].files, 0);
    }

    #[test]
    fn an_unknown_token_is_a_problem_naming_the_token_and_the_file() {
        let t = good();
        t.write("react/src/App.tsx", "// {__nope__}\n");
        let r = check(&t);
        assert!(!r.ok());
        let m = &r.problems[0].message;
        assert!(m.contains("nope") && m.contains("App.tsx"), "{m}");
    }

    #[test]
    fn unused_prompts_and_unused_files_are_notes_and_not_problems() {
        let t = good();
        t.write("stray/readme.txt", "nothing refers to this folder\n");
        t.write("manifest.json", &GOOD.replace("\"prompts\": [", "\"prompts\": [ { \"key\": \"unused\", \"type\": \"text\" },"));
        let r = check(&t);
        assert!(r.ok(), "{r}");
        let notes = r.notes.join("\n");
        assert!(notes.contains("stray/readme.txt"), "{notes}");
        assert!(notes.contains("the prompt 'unused' is not used"), "{notes}");
        // a prompt used only in a condition, or only through a derived casing, is used
        assert!(!notes.contains("'framework'") && !notes.contains("'install'") && !notes.contains("'projectName'"), "{notes}");
    }

    #[test]
    fn answers_the_caller_gives_replace_the_sample_ones_and_fix_that_prompt() {
        let t = good();
        let mut answers = Answers::new();
        answers.insert("framework".into(), json!("vue"));
        let r = check_template(&DirSource::new(&t.0), &CheckOptions { answers, ..CheckOptions::default() });
        let labels: Vec<&str> = r.variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, ["defaults", "install = true"], "the fixed prompt is not varied");
        assert!(r.ok(), "{r}");

        // an answer that breaks a rule is reported like any other problem
        let mut bad = Answers::new();
        bad.insert("projectName".into(), json!("Not Valid"));
        let r = check_template(&DirSource::new(&t.0), &CheckOptions { answers: bad, ..CheckOptions::default() });
        assert!(!r.ok() && r.problems[0].message.contains("projectName"), "{r}");
    }

    #[test]
    fn what_the_plan_warns_about_is_carried_into_the_variant_and_limits_apply() {
        let t = good();
        t.write("base/package.json", "{ \"name\": \"{__projectName__}\", \"scripts\": { \"postinstall\": \"node x.js\" } }\n");
        let r = check(&t);
        assert!(r.ok(), "warnings are not problems: {r}");
        assert!(r.variants[0].warnings.iter().any(|w| w.contains("postinstall")), "{:?}", r.variants[0].warnings);
        assert!(r.to_string().contains("warning: package.json has scripts"), "{r}");

        let tiny = CheckOptions { limits: Limits { max_files: 1, ..Limits::default() }, ..CheckOptions::default() };
        let r = check_template(&DirSource::new(&t.0), &tiny);
        assert!(!r.ok() && r.problems[0].message.contains("more than 1 files"), "{r}");
    }

    #[test]
    fn a_template_with_too_many_variants_says_so() {
        let t = good();
        let r = check_template(&DirSource::new(&t.0), &CheckOptions { max_variants: 2, ..CheckOptions::default() });
        assert_eq!(r.variants.len(), 2);
        assert!(r.notes.iter().any(|n| n.contains("only the first 2")), "{:?}", r.notes);
    }

    #[test]
    fn a_folder_that_already_exists_where_the_plan_would_go_is_not_a_problem_for_a_check() {
        // the plan is dry: it looks at no folder, so a name that is taken on this machine does not matter
        let t = good();
        let mut answers = Answers::new();
        answers.insert("projectName".into(), json!("tmp")); // the name of a folder that exists in many places
        let r = check_template(&DirSource::new(&t.0), &CheckOptions { answers, ..CheckOptions::default() });
        assert!(r.ok(), "{r}");
    }
}
