//! The merges of the add mode: edit a JSON file that already exists in a
//! solution by adding what the template has and the solution lacks.
//!
//! The rule for every helper is **only add**. A value the solution already has
//! is never changed (the official SPFx CLI lets the template win on some scalars
//! in `config.json` and `serve.json`, which would overwrite a developer's own
//! `initialPage`; this does not). New entries go after the existing ones. The
//! result keeps the file's indent, line endings, final newline and key order
//! (see `serialize_like`), so a diff shows only the added entries. A merge that
//! adds nothing returns the original bytes untouched.
//!
//! The files must be plain JSON: comments and trailing commas are refused with a
//! clear message, not dropped.

use crate::detect::{clean_version, major_minor};
use crate::error::{Result, ScaffoldError};
use crate::manifest::MergeHelper;
use crate::plan::serialize_like;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// Keys that make a feature more than the shell every component repeats.
const FEATURE_SHELL: &[&str] = &["title", "description", "id", "version"];

pub(crate) fn merge_file(helper: MergeHelper, path: &str, existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>> {
    let fail = |reason: String| ScaffoldError::Merge { file: path.to_string(), reason };

    let existing_text = String::from_utf8(existing.to_vec()).map_err(|_| fail("the file is not UTF-8 text".into()))?;
    let bom = existing_text.starts_with('\u{feff}');
    let existing_text = existing_text.trim_start_matches('\u{feff}');
    let incoming_text = String::from_utf8(incoming.to_vec()).map_err(|_| fail("the template file is not UTF-8 text".into()))?;

    let mut doc: Value = serde_json::from_str(existing_text).map_err(|e| {
        fail(format!("not valid JSON (comments and trailing commas are not supported): {e}"))
    })?;
    let inc: Value = serde_json::from_str(incoming_text.trim_start_matches('\u{feff}'))
        .map_err(|e| fail(format!("the template file is not valid JSON: {e}")))?;
    let before = doc.clone();

    if !doc.is_object() || !inc.is_object() {
        return Err(fail("expected a JSON object at the top level".into()));
    }
    match helper {
        MergeHelper::PackageJson => package_json(path, &mut doc, &inc)?,
        MergeHelper::SpfxConfig => add_by_key(&mut doc, &inc, &["bundles", "localizedResources", "externals"]),
        MergeHelper::SpfxServe => add_by_key(&mut doc, &inc, &["serveConfigurations"]),
        MergeHelper::SpfxPackageSolution => package_solution(&mut doc, &inc),
    }

    if doc == before {
        return Ok(existing.to_vec());
    }
    let mut out = serialize_like(existing_text, &doc).map_err(fail)?;
    if bom {
        out.insert(0, '\u{feff}');
    }
    Ok(out.into_bytes())
}

/// Top-level keys the file lacks are added; for the keys in `maps` that are
/// objects on both sides, the entries the file lacks are added.
fn add_by_key(doc: &mut Value, inc: &Value, maps: &[&str]) {
    let (Some(doc), Some(inc)) = (doc.as_object_mut(), inc.as_object()) else { return };
    for (key, value) in inc {
        match (maps.contains(&key.as_str()), value, doc.get_mut(key)) {
            (true, Value::Object(entries), Some(Value::Object(have))) => {
                for (name, entry) in entries {
                    if !have.contains_key(name) {
                        have.insert(name.clone(), entry.clone());
                    }
                }
            }
            (_, _, None) => {
                doc.insert(key.clone(), value.clone());
            }
            _ => {} // already there: keep the solution's value
        }
    }
}

fn package_json(path: &str, doc: &mut Value, inc: &Value) -> Result<()> {
    // One SPFx minor version per solution: another one is a different generator.
    let have = microsoft_versions(doc);
    let mismatches: Vec<String> = microsoft_versions(inc)
        .into_iter()
        .filter_map(|(name, new)| {
            let old = have.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone())?;
            let (a, b) = (clean_version(&old), clean_version(&new));
            let same = match (a.as_deref().and_then(major_minor), b.as_deref().and_then(major_minor)) {
                (Some(x), Some(y)) => x == y,
                _ => old == new,
            };
            (!same).then(|| format!("{name}: the solution has {old}, the template {new}"))
        })
        .collect();
    if !mismatches.is_empty() {
        return Err(ScaffoldError::Incompatible(format!(
            "SPFx version mismatch in {path}: all components of a solution must use the same SPFx version.\n  {}",
            mismatches.join("\n  ")
        )));
    }

    let present: HashSet<String> = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|s| doc.get(*s).and_then(Value::as_object))
        .flat_map(|m| m.keys().cloned())
        .collect();
    let root = doc.as_object_mut().expect("checked: an object");
    for section in ["dependencies", "devDependencies"] {
        let Some(wanted) = inc.get(section).and_then(Value::as_object) else { continue };
        for (name, version) in wanted {
            if present.contains(name) {
                continue; // the solution's version stays
            }
            let target = root
                .entry(section.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(target) = target.as_object_mut() {
                target.insert(name.clone(), version.clone());
            }
        }
    }
    Ok(())
}

/// `(name, range)` of every `@microsoft/*` package in dependencies and devDependencies.
fn microsoft_versions(doc: &Value) -> Vec<(String, String)> {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|s| doc.get(*s).and_then(Value::as_object))
        .flat_map(|m| m.iter())
        .filter(|(k, _)| k.starts_with("@microsoft/"))
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect()
}

fn package_solution(doc: &mut Value, inc: &Value) {
    let Some(inc_solution) = inc.get("solution").and_then(Value::as_object) else { return };
    let root = doc.as_object_mut().expect("checked: an object");
    let Some(Value::Object(solution)) = root.get_mut("solution") else {
        if !root.contains_key("solution") {
            root.insert("solution".into(), Value::Object(inc_solution.clone()));
        }
        return;
    };

    // The solution's own metadata (name, id, version, developer, paths) stays.
    for (key, value) in inc_solution {
        if key != "features" && !solution.contains_key(key) {
            solution.insert(key.clone(), value.clone());
        }
    }

    let Some(new_features) = inc_solution.get("features").and_then(Value::as_array) else { return };
    match solution.get_mut("features") {
        Some(Value::Array(have)) => {
            let ids: HashSet<String> = have
                .iter()
                .filter_map(|f| f.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            for feature in new_features {
                let id = feature.get("id").and_then(Value::as_str).unwrap_or_default();
                if ids.contains(id) {
                    continue;
                }
                // Every component brings a feature with a fresh id and nothing in it.
                // Those add nothing the solution's own feature does not do, so only a
                // feature that carries something (element manifests, assets) is added.
                let carries_something = feature
                    .as_object()
                    .is_some_and(|f| f.keys().any(|k| !FEATURE_SHELL.contains(&k.as_str())));
                if carries_something || have.is_empty() {
                    have.push(feature.clone());
                }
            }
        }
        Some(_) => {}
        None => {
            solution.insert("features".into(), Value::Array(new_features.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(helper: MergeHelper, existing: &str, incoming: &str) -> Result<String> {
        merge_file(helper, "f.json", existing.as_bytes(), incoming.as_bytes()).map(|b| String::from_utf8(b).unwrap())
    }

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    // ---------------------------------------------------------- package.json

    const PKG_HAVE: &str = "{\n    \"name\": \"my-sol\",\n    \"dependencies\": {\n        \"react\": \"17.0.1\",\n        \"@microsoft/sp-core-library\": \"1.24.1\"\n    },\n    \"devDependencies\": {\n        \"typescript\": \"~5.8.0\"\n    }\n}\n";
    const PKG_NEW: &str = r#"{ "name": "other", "dependencies": { "@fluentui/react": "^8.106.4", "react": "18.0.0", "@microsoft/sp-core-library": "1.24.1" },
        "devDependencies": { "@types/react": "17.0.45", "typescript": "~5.0.0" } }"#;

    #[test]
    fn package_json_adds_only_what_is_missing_after_what_is_there_and_keeps_the_format() {
        let out = run(MergeHelper::PackageJson, PKG_HAVE, PKG_NEW).unwrap();
        let doc = json(&out);
        assert_eq!(doc["name"], "my-sol", "the solution's own fields stay");
        assert_eq!(doc["dependencies"]["react"], "17.0.1", "an existing version wins");
        assert_eq!(doc["devDependencies"]["typescript"], "~5.8.0");
        assert_eq!(doc["dependencies"]["@fluentui/react"], "^8.106.4");
        assert_eq!(doc["devDependencies"]["@types/react"], "17.0.45");
        // 4-space indent kept, new entries last
        assert!(out.contains("\n        \"@fluentui/react\""), "{out}");
        let keys: Vec<&String> = doc["dependencies"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["react", "@microsoft/sp-core-library", "@fluentui/react"]);
    }

    #[test]
    fn package_json_with_nothing_to_add_comes_back_byte_for_byte() {
        let same = run(MergeHelper::PackageJson, PKG_HAVE, r#"{ "dependencies": { "react": "9.9.9" } }"#).unwrap();
        assert_eq!(same, PKG_HAVE);
    }

    #[test]
    fn a_package_that_is_in_the_other_section_is_not_added_again() {
        let out = run(
            MergeHelper::PackageJson,
            r#"{ "devDependencies": { "zod": "4.0.0" } }"#,
            r#"{ "dependencies": { "zod": "3.0.0" } }"#,
        )
        .unwrap();
        assert!(json(&out)["dependencies"].is_null());
    }

    #[test]
    fn another_spfx_minor_version_is_refused_and_another_patch_is_not() {
        let have = r#"{ "dependencies": { "@microsoft/sp-core-library": "1.24.1" } }"#;
        let err = run(MergeHelper::PackageJson, have, r#"{ "dependencies": { "@microsoft/sp-core-library": "1.23.2" } }"#)
            .unwrap_err();
        assert!(matches!(err, ScaffoldError::Incompatible(_)));
        assert!(err.to_string().contains("1.24.1") && err.to_string().contains("1.23.2"));
        // across sections too
        assert!(run(MergeHelper::PackageJson, have, r#"{ "devDependencies": { "@microsoft/sp-core-library": "1.25.0" } }"#).is_err());
        // a patch ahead is fine, the solution's version stays
        let out = run(MergeHelper::PackageJson, have, r#"{ "dependencies": { "@microsoft/sp-core-library": "1.24.3" } }"#).unwrap();
        assert_eq!(out, have);
    }

    // ----------------------------------------------------------- config.json

    #[test]
    fn config_json_adds_bundles_and_strings_and_keeps_the_solutions_scalars() {
        let have = "{\r\n  \"$schema\": \"mine\",\r\n  \"version\": \"2.0\",\r\n  \"bundles\": { \"a-web-part\": { \"components\": [] } },\r\n  \"externals\": {},\r\n  \"localizedResources\": {}\r\n}\r\n";
        let inc = r#"{ "$schema": "theirs", "version": "9", "bundles": { "a-web-part": { "components": [1] }, "b-web-part": {} },
            "localizedResources": { "BStrings": "x" } }"#;
        let out = run(MergeHelper::SpfxConfig, have, inc).unwrap();
        let doc = json(&out);
        assert_eq!(doc["$schema"], "mine");
        assert_eq!(doc["version"], "2.0");
        assert_eq!(doc["bundles"]["a-web-part"]["components"], json("[]"), "an existing bundle is not touched");
        assert!(doc["bundles"]["b-web-part"].is_object());
        assert_eq!(doc["localizedResources"]["BStrings"], "x");
        assert!(out.contains("\r\n") && !out.replace("\r\n", "").contains('\n'), "CRLF kept");
    }

    // ------------------------------------------------------------ serve.json

    #[test]
    fn serve_json_never_overwrites_the_developers_own_settings() {
        let have = r#"{ "port": 4321, "https": true, "initialPage": "https://contoso.sharepoint.com/_layouts/workbench.aspx" }"#;
        let inc = r#"{ "port": 4000, "https": false, "initialPage": "https://{tenantDomain}/_layouts/workbench.aspx",
            "serveConfigurations": { "default": { "pageUrl": "x" } } }"#;
        let doc = json(&run(MergeHelper::SpfxServe, have, inc).unwrap());
        assert_eq!(doc["initialPage"], "https://contoso.sharepoint.com/_layouts/workbench.aspx");
        assert_eq!(doc["port"], 4321);
        assert_eq!(doc["https"], true);
        assert!(doc["serveConfigurations"]["default"].is_object());
    }

    // ------------------------------------------------------ package-solution

    const SOL_HAVE: &str = r#"{ "solution": { "name": "mine", "id": "SOL", "version": "1.0.0.0",
        "features": [ { "title": "F", "description": "d", "id": "F1", "version": "1.0.0.0" } ] }, "paths": { "zippedPackage": "solution/mine.sppkg" } }"#;

    #[test]
    fn package_solution_keeps_the_solution_and_skips_the_empty_feature_every_component_brings() {
        let inc = r#"{ "solution": { "name": "theirs", "id": "OTHER", "features": [ { "title": "G", "description": "d", "id": "F2", "version": "1.0.0.0" } ] },
            "paths": { "zippedPackage": "x" } }"#;
        let out = run(MergeHelper::SpfxPackageSolution, SOL_HAVE, inc).unwrap();
        assert_eq!(out, SOL_HAVE, "nothing to add, so the file is untouched");
    }

    #[test]
    fn package_solution_adds_a_feature_that_carries_something_and_never_the_same_id_twice() {
        let rich = r#"{ "solution": { "features": [ { "title": "G", "id": "F2", "version": "1", "assets": { "elementManifests": ["e.xml"] } },
            { "title": "F", "id": "F1", "version": "1", "assets": {} } ] } }"#;
        let doc = json(&run(MergeHelper::SpfxPackageSolution, SOL_HAVE, rich).unwrap());
        let ids: Vec<&str> = doc["solution"]["features"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["F1", "F2"]);
        assert_eq!(doc["solution"]["name"], "mine");
    }

    #[test]
    fn package_solution_takes_the_features_when_the_solution_has_none() {
        let have = r#"{ "solution": { "name": "mine" } }"#;
        let inc = r#"{ "solution": { "features": [ { "title": "G", "id": "F2", "version": "1" } ] } }"#;
        let doc = json(&run(MergeHelper::SpfxPackageSolution, have, inc).unwrap());
        assert_eq!(doc["solution"]["features"][0]["id"], "F2");
    }

    // ----------------------------------------------------------------- shared

    #[test]
    fn comments_are_refused_with_a_clear_message() {
        let err = run(MergeHelper::SpfxConfig, "{\n  // note\n  \"bundles\": {}\n}\n", r#"{ "bundles": { "a": {} } }"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("f.json") && err.contains("comments"), "{err}");
    }

    #[test]
    fn a_byte_order_mark_and_a_missing_final_newline_are_kept() {
        let have = "\u{feff}{\n  \"bundles\": {}\n}";
        let out = run(MergeHelper::SpfxConfig, have, r#"{ "bundles": { "a": {} } }"#).unwrap();
        assert!(out.starts_with('\u{feff}'));
        assert!(out.ends_with('}'), "no newline was added: {out:?}");
        assert!(json(out.trim_start_matches('\u{feff}'))["bundles"]["a"].is_object());
    }

    #[test]
    fn something_that_is_not_an_object_is_an_error() {
        assert!(run(MergeHelper::SpfxServe, "[]", "{}").is_err());
    }
}
