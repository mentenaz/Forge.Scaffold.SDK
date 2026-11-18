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
    if helper == MergeHelper::SpfxElements {
        return elements_xml(path, existing, incoming);
    }

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
        MergeHelper::SpfxElements => unreachable!("handled above"),
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
                // A feature that provisions element files (extensions) joins the feature that
                // already does: the elements are merged into the same files, so a second
                // feature would provision them twice.
                if let Some(wanted) = feature.pointer("/assets/elementManifests").and_then(Value::as_array) {
                    let target = have
                        .iter_mut()
                        .find_map(|f| f.pointer_mut("/assets/elementManifests").and_then(Value::as_array_mut));
                    if let Some(list) = target {
                        for name in wanted {
                            if !list.contains(name) {
                                list.push(name.clone());
                            }
                        }
                        continue;
                    }
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

// -------------------------------------------------------------------- elements

/// `sharepoint/assets/elements.xml` and `ClientSideInstance.xml`: add the
/// template's elements inside the solution's own `<Elements>`. Text based, so
/// everything already in the file (comments, attribute order, indentation) stays.
/// An element whose component id is already in the file is not added again.
fn elements_xml(path: &str, existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>> {
    let fail = |reason: String| ScaffoldError::Merge { file: path.to_string(), reason };
    let have = String::from_utf8(existing.to_vec()).map_err(|_| fail("the file is not UTF-8 text".into()))?;
    let wanted = String::from_utf8(incoming.to_vec()).map_err(|_| fail("the template file is not UTF-8 text".into()))?;

    let inner = elements_inner(&wanted).ok_or_else(|| fail("the template file has no <Elements> with content".into()))?;
    let block = inner.trim_start_matches(['\r', '\n']).trim_end().replace("\r\n", "\n");
    if block.trim().is_empty() {
        return Ok(existing.to_vec());
    }
    let close = have
        .rfind("</Elements>")
        .ok_or_else(|| fail("not an <Elements> file: there is no closing </Elements>".into()))?;

    if component_ids(&block).iter().any(|id| have.contains(id.as_str())) {
        return Ok(existing.to_vec()); // this component is already provisioned
    }

    let mut out = format!("{}\n{block}\n{}", have[..close].trim_end(), &have[close..]);
    if have.contains("\r\n") {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Ok(out.into_bytes())
}

/// What is between `<Elements ...>` and `</Elements>`.
fn elements_inner(text: &str) -> Option<&str> {
    let open = text.find("<Elements")?;
    let start = open + text[open..].find('>')? + 1;
    let end = text.rfind("</Elements>")?;
    (start <= end).then(|| &text[start..end])
}

/// The values of every `ComponentId="..."` (which includes `ClientSideComponentId`).
fn component_ids(text: &str) -> Vec<String> {
    let needle = "ComponentId=\"";
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(needle) {
        let after = &rest[i + needle.len()..];
        match after.find('"') {
            Some(end) => {
                ids.push(after[..end].to_string());
                rest = &after[end..];
            }
            None => break,
        }
    }
    ids
}

/// The value of attribute `name` in one tag, if it is an attribute of its own
/// (`ID=` and not the end of `ComponentId=`).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let mut from = 0;
    while let Some(i) = tag[from..].find(&needle) {
        let at = from + i;
        if tag[..at].chars().next_back().is_some_and(char::is_whitespace) {
            let start = at + needle.len();
            let end = tag[start..].find('"')?;
            return Some(&tag[start..start + end]);
        }
        from = at + needle.len();
    }
    None
}

/// Warnings about an elements file after a merge: two elements with the same
/// `ID="..."`, or two `<Field>`s with the same `Name` (the field customizer
/// templates ship a sample column, so two of them define the same one).
pub(crate) fn elements_warnings(path: &str, text: &str) -> Vec<String> {
    let mut out = elements_duplicate_ids(path, text);
    let mut names: HashSet<String> = HashSet::new();
    let mut warned: HashSet<String> = HashSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("<Field") {
        let tag_text = &rest[i..];
        let end = tag_text.find('>').unwrap_or(tag_text.len());
        if let Some(name) = attribute(&tag_text[..end], "Name") {
            if !names.insert(name.to_string()) && warned.insert(name.to_string()) {
                out.push(format!(
                    "{path} now has two fields named {name}: change one before deploying, because the name of a site column must be unique"
                ));
            }
        }
        rest = &tag_text[end..];
    }
    out
}

fn elements_duplicate_ids(path: &str, text: &str) -> Vec<String> {
    let needle = "ID=\"";
    let mut seen: HashSet<String> = HashSet::new();
    let mut warned: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(needle) {
        let at = from + i;
        let value_start = at + needle.len();
        let Some(end) = text[value_start..].find('"') else { break };
        let value = text[value_start..value_start + end].to_ascii_lowercase();
        let attribute_start = text[..at].chars().next_back().is_some_and(char::is_whitespace);
        if attribute_start && !seen.insert(value.clone()) && warned.insert(value.clone()) {
            out.push(format!(
                "{path} now has two elements with the ID {value}: change one before deploying, or SharePoint sees the same field twice"
            ));
        }
        from = value_start + end;
    }
    out
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

    // --------------------------------------------------------------- elements

    const ELEMENTS_HAVE: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Elements xmlns=\"http://schemas.microsoft.com/sharepoint/\">\n    <CustomAction\n        Title=\"BannerApplicationCustomizer\"\n        ClientSideComponentId=\"AAA\">\n    </CustomAction>\n</Elements>\n";
    const ELEMENTS_NEW: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Elements xmlns=\"http://schemas.microsoft.com/sharepoint/\">\n    <Field ID=\"{f1}\"\n            Name=\"Rating\"\n            ClientSideComponentId=\"BBB\">\n    </Field>\n</Elements>\n";

    fn xml(existing: &str, incoming: &str) -> Result<String> {
        merge_file(MergeHelper::SpfxElements, "sharepoint/assets/elements.xml", existing.as_bytes(), incoming.as_bytes())
            .map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn elements_are_added_inside_the_existing_root_and_everything_else_stays() {
        let out = xml(ELEMENTS_HAVE, ELEMENTS_NEW).unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Elements xmlns=\"http://schemas.microsoft.com/sharepoint/\">\n    <CustomAction\n        Title=\"BannerApplicationCustomizer\"\n        ClientSideComponentId=\"AAA\">\n    </CustomAction>\n    <Field ID=\"{f1}\"\n            Name=\"Rating\"\n            ClientSideComponentId=\"BBB\">\n    </Field>\n</Elements>\n"
        );
    }

    #[test]
    fn a_component_that_is_already_in_the_file_is_not_added_twice() {
        assert_eq!(xml(ELEMENTS_HAVE, ELEMENTS_HAVE).unwrap(), ELEMENTS_HAVE);
        let once = xml(ELEMENTS_HAVE, ELEMENTS_NEW).unwrap();
        assert_eq!(xml(&once, ELEMENTS_NEW).unwrap(), once);
    }

    #[test]
    fn elements_keep_crlf_line_endings() {
        let crlf = ELEMENTS_HAVE.replace('\n', "\r\n");
        let out = xml(&crlf, ELEMENTS_NEW).unwrap();
        assert!(out.contains("BBB") && !out.replace("\r\n", "").contains('\n'), "{out:?}");
    }

    #[test]
    fn a_component_instance_file_is_merged_by_its_component_id() {
        let have = "<Elements xmlns=\"x\">\n    <ClientSideComponentInstance Title=\"A\"\n        ComponentId=\"AAA\">\n    </ClientSideComponentInstance>\n</Elements>\n";
        let new = "<Elements xmlns=\"x\">\n    <ClientSideComponentInstance Title=\"B\"\n        ComponentId=\"BBB\">\n    </ClientSideComponentInstance>\n</Elements>\n";
        let out = xml(have, new).unwrap();
        assert!(out.contains("AAA") && out.contains("BBB"));
        assert_eq!(xml(&out, new).unwrap(), out);
    }

    #[test]
    fn something_that_is_not_an_elements_file_is_an_error() {
        assert!(xml("<Other/>", ELEMENTS_NEW).is_err());
        assert!(xml(ELEMENTS_HAVE, "<Other/>").is_err());
    }

    #[test]
    fn two_elements_with_the_same_id_are_reported() {
        let two = "<Elements>\n  <Field ID=\"{F1}\" ClientSideComponentId=\"A\"/>\n  <Field ID=\"{f1}\" ClientSideComponentId=\"B\"/>\n</Elements>";
        let w = elements_warnings("elements.xml", two);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("{f1}"));
        assert!(elements_warnings("elements.xml", ELEMENTS_NEW).is_empty());
        assert!(elements_warnings("elements.xml", "<Elements><CustomAction ClientSideComponentId=\"A\"/><CustomAction ClientSideComponentId=\"A\"/></Elements>").is_empty(), "ComponentId is not an ID");
    }

    #[test]
    fn two_fields_with_the_same_name_are_reported() {
        let two = "<Elements>\n  <Field ID=\"{a}\" Name=\"SPFxPercentage\" ClientSideComponentId=\"A\"/>\n  <Field ID=\"{b}\" Name=\"SPFxPercentage\" ClientSideComponentId=\"B\"/>\n</Elements>";
        let w = elements_warnings("elements.xml", two);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("SPFxPercentage"));
        let ok = "<Elements>\n  <Field ID=\"{a}\" Name=\"One\"/>\n  <Field ID=\"{b}\" Name=\"Two\"/>\n  <CustomAction Name=\"One\"/>\n</Elements>";
        assert!(elements_warnings("elements.xml", ok).is_empty(), "only Field names count");
    }

    #[test]
    fn package_solution_adds_extensions_to_the_feature_that_already_provisions_element_files() {
        let have = r#"{ "solution": { "name": "mine", "features": [
            { "title": "Shell", "id": "S", "version": "1" },
            { "title": "Ext", "id": "E1", "version": "1", "assets": { "elementManifests": ["elements.xml", "ClientSideInstance.xml"] } } ] } }"#;
        let incoming = r#"{ "solution": { "features": [
            { "title": "Field", "id": "E2", "version": "1", "assets": { "elementManifests": ["elements.xml", "Other.xml"] } } ] } }"#;
        let doc = json(&run(MergeHelper::SpfxPackageSolution, have, incoming).unwrap());
        let features = doc["solution"]["features"].as_array().unwrap();
        assert_eq!(features.len(), 2, "no second feature for the same element files");
        assert_eq!(features[1]["assets"]["elementManifests"], json(r#"["elements.xml", "ClientSideInstance.xml", "Other.xml"]"#));
    }
}
