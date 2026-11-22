//! Editing a few single values of an existing SPFx solution: the name SharePoint shows, the description, and the version.
//!
//! Everything here is in `config/package-solution.json`, edited as text so its comments, trailing commas, indentation and
//! line endings stay as they are. The result is an [`AddPlan`], so it is applied with [`apply_edit`] (the same safe writer
//! as an add: it checks nothing changed since the plan, writes beside the file and renames, and puts everything back on a
//! failed write), shows the same preview, and is undone with the same record: use
//! `plan.undo_record_as("edit", label)` **before** applying, and [`crate::revert_edit`] later.
//!
//! What can be edited is deliberately small. The solution id is never changed (it would break upgrades of a deployed
//! solution), and neither is the SPFx version (that is an upgrade), the build system or the framework.

use crate::add::{apply_add, goes_through_a_link, AddFile, AddOutcome, AddPlan};
use crate::detect::SpfxSolution;
use crate::error::{Result, ScaffoldError};
use crate::jsonc::{Doc, Seg};
use crate::stage::{WriteOutcome, WriteProgress};
use crate::validate::ValidationIssue;
use serde_json::Value;
use std::fs;

const FILE: &str = "config/package-solution.json";
const MAX_NAME: usize = 255;
const MAX_DESCRIPTION: usize = 1000;

/// The values to change; a `None` is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SolutionEdit {
    /// `solution.name`: the name SharePoint shows for the solution package.
    pub name: Option<String>,
    /// The description of the solution's first feature (`solution.features[0].description`). Other features are not touched.
    pub description: Option<String>,
    /// `solution.version`: four numbers, `1.0.0.0`. `package.json` is not changed.
    pub version: Option<String>,
}

fn check(edit: &SolutionEdit) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut issue = |field: &str, message: String| issues.push(ValidationIssue::new(field, message));
    let control = |s: &str| s.chars().any(|c| c.is_control());
    if let Some(n) = &edit.name {
        if n.trim().is_empty() {
            issue("name", "the name cannot be empty".into());
        } else if control(n) {
            issue("name", "the name cannot contain control characters or line breaks".into());
        } else if n.chars().count() > MAX_NAME {
            issue("name", format!("the name can be at most {MAX_NAME} characters"));
        }
    }
    if let Some(d) = &edit.description {
        if control(d) {
            issue("description", "the description cannot contain control characters or line breaks".into());
        } else if d.chars().count() > MAX_DESCRIPTION {
            issue("description", format!("the description can be at most {MAX_DESCRIPTION} characters"));
        }
    }
    if let Some(v) = &edit.version {
        let parts: Vec<&str> = v.split('.').collect();
        let ok = parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| n <= 65535));
        if !ok {
            issue("version", "the version must be four numbers from 0 to 65535, like 1.0.0.0".into());
        }
    }
    issues
}

/// Plans the edit of the solution's `config/package-solution.json`. Writes nothing. A value that is already what was asked
/// for is not a change, so an edit that changes nothing gives an empty plan ([`AddPlan::is_empty`]).
pub fn plan_edit(solution: &SpfxSolution, edit: &SolutionEdit) -> Result<AddPlan> {
    let issues = check(edit);
    if !issues.is_empty() {
        return Err(ScaffoldError::Validation(issues));
    }
    let root = &solution.root;
    if goes_through_a_link(root, FILE) {
        return Err(ScaffoldError::LinkInSolution(FILE.to_string()));
    }
    let merge_err = |reason: String| ScaffoldError::Merge { file: FILE.to_string(), reason };
    let before = fs::read(root.join(FILE)).map_err(|e| merge_err(format!("cannot be read: {e}")))?;
    let text = String::from_utf8(before.clone()).map_err(|_| merge_err("is not UTF-8 text".into()))?;
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text.as_str()),
    };
    let mut doc = Doc::open(body).map_err(|e| merge_err(format!("is not valid JSON: {e}")))?;
    let current = doc.value().map_err(merge_err)?;

    let mut set = |path: &[Seg], pointer: &str, value: &Option<String>, what: &str| -> Result<()> {
        let Some(new) = value else { return Ok(()) };
        match current.pointer(pointer) {
            Some(Value::String(old)) if old == new => Ok(()),
            Some(Value::String(_)) => doc.replace_value(path, &Value::String(new.clone())).map_err(merge_err),
            _ => Err(merge_err(format!("has no {what} to change"))),
        }
    };
    let solution_key = || Seg::key("solution");
    set(&[solution_key(), Seg::key("name")], "/solution/name", &edit.name, "solution name")?;
    set(&[solution_key(), Seg::key("version")], "/solution/version", &edit.version, "solution version")?;
    set(
        &[solution_key(), Seg::key("features"), Seg::Index(0), Seg::key("description")],
        "/solution/features/0/description",
        &edit.description,
        "feature description (solution.features[0].description)",
    )?;

    let after = format!("{bom}{}", doc.into_text()).into_bytes();
    let mut files = Vec::new();
    if after != before {
        files.push(AddFile { path: FILE.to_string(), outcome: AddOutcome::Merged, content: after, previous: Some(before) });
    }
    Ok(AddPlan { root: root.clone(), files, kept: Vec::new(), warnings: solution.warnings.clone() })
}

/// Applies an edit plan. This is [`apply_add`]: the same checks and the same rollback.
pub fn apply_edit(plan: &AddPlan, on_progress: impl FnMut(WriteProgress)) -> Result<WriteOutcome> {
    apply_add(plan, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::BuildSystem;
    use crate::undo::{revert_edit, UndoRecord};
    use std::path::PathBuf;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-edit-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// As the generators write it: comments, a trailing comma, CRLF line endings, two features.
    const PS: &str = "{\r\n  // the solution\r\n  \"$schema\": \"x\",\r\n  \"solution\": {\r\n    \"name\": \"my-sol-client-side-solution\",\r\n    \"id\": \"11111111-2222-3333-4444-555555555555\",\r\n    \"version\": \"1.0.0.0\", /* bump on release */\r\n    \"features\": [\r\n      {\r\n        \"title\": \"first\",\r\n        \"description\": \"first description\",\r\n      },\r\n      {\r\n        \"title\": \"second\",\r\n        \"description\": \"second description\"\r\n      }\r\n    ]\r\n  },\r\n  \"paths\": { \"zippedPackage\": \"solution/x.sppkg\" }\r\n}\r\n";

    fn setup(ps: &str) -> (Tmp, SpfxSolution) {
        let t = Tmp::new();
        t.write("package.json", "{ \"name\": \"my-sol\", \"version\": \"0.0.1\" }\n");
        t.write(FILE, ps);
        let solution = SpfxSolution {
            root: t.0.clone(),
            version: Some("1.21.1".into()),
            build_system: BuildSystem::Gulp,
            framework: None,
            package_name: Some("my-sol".into()),
            node_range: None,
            marker: None,
            warnings: Vec::new(),
        };
        (t, solution)
    }

    fn edit(name: Option<&str>, description: Option<&str>, version: Option<&str>) -> SolutionEdit {
        SolutionEdit { name: name.map(Into::into), description: description.map(Into::into), version: version.map(Into::into) }
    }

    #[test]
    fn changes_only_the_values_asked_for_and_keeps_the_rest_of_the_file_as_it_was() {
        let (t, sol) = setup(PS);
        let plan = plan_edit(&sol, &edit(Some("Intranet"), Some("The intranet"), Some("2.1.0.5"))).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].outcome, AddOutcome::Merged);
        apply_edit(&plan, |_| {}).unwrap();
        let after = t.read(FILE);
        let expected = PS
            .replace("my-sol-client-side-solution", "Intranet")
            .replace("\"1.0.0.0\"", "\"2.1.0.5\"")
            .replace("first description", "The intranet");
        assert_eq!(after, expected, "only the three values differ: comments, commas, CRLF and the second feature are untouched");
        assert_eq!(t.read("package.json"), "{ \"name\": \"my-sol\", \"version\": \"0.0.1\" }\n", "package.json is not changed");
        assert!(after.contains("11111111-2222-3333-4444-555555555555"), "the id is never changed");
    }

    #[test]
    fn a_value_left_out_or_already_the_same_is_not_a_change() {
        let (_t, sol) = setup(PS);
        assert!(plan_edit(&sol, &SolutionEdit::default()).unwrap().is_empty());
        assert!(plan_edit(&sol, &edit(Some("my-sol-client-side-solution"), Some("first description"), Some("1.0.0.0"))).unwrap().is_empty());
        let plan = plan_edit(&sol, &edit(None, None, Some("1.0.0.1"))).unwrap();
        assert_eq!(plan.files.len(), 1);
    }

    #[test]
    fn text_that_needs_escaping_is_written_as_valid_json() {
        let (t, sol) = setup(PS);
        let plan = plan_edit(&sol, &edit(Some(r#"The "best" \ solution"#), None, None)).unwrap();
        apply_edit(&plan, |_| {}).unwrap();
        let doc = Doc::open(&t.read(FILE)).unwrap().value().unwrap();
        assert_eq!(doc["solution"]["name"], r#"The "best" \ solution"#);
    }

    #[test]
    fn bad_values_are_all_reported_and_nothing_is_planned() {
        let (_t, sol) = setup(PS);
        let err = plan_edit(&sol, &edit(Some("  "), Some("two\nlines"), Some("1.0.0"))).unwrap_err();
        match err {
            ScaffoldError::Validation(issues) => {
                let fields: Vec<&str> = issues.iter().map(|i| i.field.as_str()).collect();
                assert_eq!(fields, ["name", "description", "version"]);
            }
            other => panic!("{other}"),
        }
        for v in ["1.0.0.0.0", "1.0.0.x", "1.0.0.70000", "1..0.0", ""] {
            assert!(plan_edit(&sol, &edit(None, None, Some(v))).is_err(), "{v:?}");
        }
        assert!(plan_edit(&sol, &edit(None, None, Some("65535.0.0.1"))).is_ok());
    }

    #[test]
    fn a_file_without_the_value_to_change_is_an_error_not_a_guess() {
        let (_t, sol) = setup("{ \"solution\": { \"name\": \"s\", \"id\": \"x\", \"version\": \"1.0.0.0\" } }");
        let err = plan_edit(&sol, &edit(None, Some("d"), None)).unwrap_err();
        assert!(err.to_string().contains("no feature description"), "{err}");
        assert!(plan_edit(&sol, &edit(Some("n"), None, None)).is_ok(), "the name is there");
        let (_t2, broken) = setup("{ not json");
        assert!(plan_edit(&broken, &edit(Some("n"), None, None)).unwrap_err().to_string().contains("not valid JSON"));
    }

    #[test]
    fn a_byte_order_mark_is_kept() {
        let (t, sol) = setup(&format!("\u{feff}{PS}"));
        apply_edit(&plan_edit(&sol, &edit(Some("N"), None, None)).unwrap(), |_| {}).unwrap();
        assert!(t.read(FILE).starts_with('\u{feff}'));
    }

    #[test]
    fn an_edit_is_undone_with_the_same_record_and_a_later_change_blocks_it() {
        let (t, sol) = setup(PS);
        let plan = plan_edit(&sol, &edit(Some("Intranet"), None, Some("1.1.0.0"))).unwrap();
        let record = plan.undo_record_as("edit", "Renamed to Intranet, version 1.1.0.0");
        assert_eq!(record.kind, "edit");
        apply_edit(&plan, |_| {}).unwrap();
        let stored = record.to_json();

        // the file is changed by hand afterwards: the revert refuses and names it
        t.write(FILE, &format!("{}\r\n// my note\r\n", t.read(FILE)));
        let err = revert_edit(&t.0, &UndoRecord::from_json(&stored).unwrap()).unwrap_err();
        assert!(matches!(err, ScaffoldError::RevertConflict(ref f) if f == &[FILE]), "{err}");

        // a fresh edit and its revert give the file back byte for byte
        t.write(FILE, PS);
        let plan = plan_edit(&sol, &edit(Some("Intranet"), None, Some("1.1.0.0"))).unwrap();
        let record = plan.undo_record_as("edit", "x");
        apply_edit(&plan, |_| {}).unwrap();
        revert_edit(&t.0, &record).unwrap();
        assert_eq!(t.read(FILE), PS);
    }

    #[test]
    fn the_file_is_not_edited_if_it_changed_after_the_plan() {
        let (t, sol) = setup(PS);
        let plan = plan_edit(&sol, &edit(Some("Intranet"), None, None)).unwrap();
        t.write(FILE, &PS.replace("1.0.0.0", "9.9.9.9"));
        let err = apply_edit(&plan, |_| {}).unwrap_err();
        assert!(matches!(err, ScaffoldError::ChangedSincePlan(_)), "{err}");
        assert!(t.read(FILE).contains("9.9.9.9") && !t.read(FILE).contains("Intranet"));
    }
}
