//! Token engine: `{__name__}` replacement in file paths and file contents.
//!
//! Every name token also gets derived forms: `name`, `namePascal`,
//! `nameCamel` and `nameKebab`. GUID tokens are stable within one scope
//! (the same `{__webpartId__}` gives the same value everywhere in one
//! iteration), which is what SPFx manifests need.

use crate::error::{Result, ScaffoldError};
use std::collections::BTreeMap;

// ---------------------------------------------------------------- casing

/// Splits `HelloWorld`, `hello-world`, `hello world`, `APIThing` into words.
pub fn split_words(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();

    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let lower_to_upper =
                (prev.is_lowercase() || prev.is_ascii_digit()) && c.is_uppercase();
            let acronym_end = prev.is_uppercase()
                && c.is_uppercase()
                && next.is_some_and(|n| n.is_lowercase());
            if lower_to_upper || acronym_end {
                words.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut it = word.chars();
    match it.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &it.as_str().to_lowercase(),
        None => String::new(),
    }
}

pub fn to_pascal(input: &str) -> String {
    split_words(input).iter().map(|w| capitalize(w)).collect()
}

pub fn to_camel(input: &str) -> String {
    let words = split_words(input);
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&w.to_lowercase());
        } else {
            out.push_str(&capitalize(w));
        }
    }
    out
}

pub fn to_kebab(input: &str) -> String {
    split_words(input)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

// ------------------------------------------------------------------ GUIDs

/// Source of GUIDs. Swap in [`SequentialGuids`] for deterministic tests.
pub trait GuidSource {
    fn next_guid(&mut self) -> String;
}

/// Random v4 GUIDs (lowercase, hyphenated, as used in SPFx manifests).
pub struct RandomGuids;

impl GuidSource for RandomGuids {
    fn next_guid(&mut self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

/// Deterministic GUIDs: `00000000-0000-0000-0000-000000000001`, `...02`, ...
pub struct SequentialGuids {
    next: u128,
}

impl SequentialGuids {
    pub fn new() -> Self {
        Self { next: 1 }
    }
}

impl Default for SequentialGuids {
    fn default() -> Self {
        Self::new()
    }
}

impl GuidSource for SequentialGuids {
    fn next_guid(&mut self) -> String {
        let id = uuid::Uuid::from_u128(self.next).to_string();
        self.next += 1;
        id
    }
}

// --------------------------------------------------------------- TokenMap

#[derive(Debug, Clone, Default)]
pub struct TokenMap {
    map: BTreeMap<String, String>,
}

/// A token name must be non-empty ASCII letters and digits.
pub fn is_valid_token_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric())
}

fn check_name(name: &str) -> Result<()> {
    if !is_valid_token_name(name) {
        return Err(ScaffoldError::InvalidTokenName(name.to_string()));
    }
    Ok(())
}

impl TokenMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets one exact token (no derived forms).
    pub fn insert(&mut self, name: &str, value: impl Into<String>) -> Result<()> {
        check_name(name)?;
        self.map.insert(name.to_string(), value.into());
        Ok(())
    }

    /// Sets `name`, `namePascal`, `nameCamel` and `nameKebab`.
    pub fn insert_name(&mut self, name: &str, value: &str) -> Result<()> {
        self.insert(name, value)?;
        self.insert(&format!("{name}Pascal"), to_pascal(value))?;
        self.insert(&format!("{name}Camel"), to_camel(value))?;
        self.insert(&format!("{name}Kebab"), to_kebab(value))?;
        Ok(())
    }

    /// Generates one GUID and stores it under `name`.
    pub fn insert_guid(&mut self, name: &str, source: &mut dyn GuidSource) -> Result<()> {
        let guid = source.next_guid();
        self.insert(name, guid)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(String::as_str)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
}

// ----------------------------------------------------------------- render

/// Replaces every `{__token__}` in `input`. `context` is only used in error
/// messages (e.g. the file path being rendered).
///
/// Unknown tokens are an error rather than being left in place, so a typo in a
/// template is caught at plan time. Text that does not have the token shape
/// (e.g. `{__ x __}`) is left untouched.
pub fn render(input: &str, tokens: &TokenMap, context: &str) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("{__") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];

        let Some(end) = after.find("__}") else {
            // No closing marker anywhere: the rest is plain text.
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };

        let name = &after[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            // Not token-shaped: emit the opener literally and keep scanning.
            out.push_str("{__");
            rest = after;
            continue;
        }

        let value = tokens.get(name).ok_or_else(|| ScaffoldError::UnknownToken {
            token: name.to_string(),
            context: context.to_string(),
        })?;
        out.push_str(value);
        rest = &after[end + 3..];
    }

    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casing_handles_common_shapes() {
        assert_eq!(to_pascal("hello world"), "HelloWorld");
        assert_eq!(to_pascal("hello-world"), "HelloWorld");
        assert_eq!(to_pascal("HelloWorld"), "HelloWorld");
        assert_eq!(to_camel("HelloWorld"), "helloWorld");
        assert_eq!(to_kebab("HelloWorld"), "hello-world");
        assert_eq!(to_kebab("hello_world"), "hello-world");
    }

    #[test]
    fn casing_handles_acronyms_and_digits() {
        assert_eq!(split_words("APIThing"), vec!["API", "Thing"]);
        assert_eq!(to_kebab("APIThing"), "api-thing");
        assert_eq!(split_words("chart2Web"), vec!["chart2", "Web"]);
    }

    #[test]
    fn insert_name_adds_derived_forms() {
        let mut t = TokenMap::new();
        t.insert_name("webpartNaam", "HelloWorld").unwrap();
        assert_eq!(t.get("webpartNaam"), Some("HelloWorld"));
        assert_eq!(t.get("webpartNaamPascal"), Some("HelloWorld"));
        assert_eq!(t.get("webpartNaamCamel"), Some("helloWorld"));
        assert_eq!(t.get("webpartNaamKebab"), Some("hello-world"));
    }

    #[test]
    fn render_replaces_in_paths_and_content() {
        let mut t = TokenMap::new();
        t.insert_name("webpartNaam", "HelloWorld").unwrap();
        assert_eq!(
            render("src/webparts/{__webpartNaam__}/{__webpartNaam__}.tsx", &t, "path").unwrap(),
            "src/webparts/HelloWorld/HelloWorld.tsx"
        );
        assert_eq!(
            render("const a = `${x}`; // {__webpartNaamCamel__}", &t, "file").unwrap(),
            "const a = `${x}`; // helloWorld"
        );
    }

    #[test]
    fn render_rejects_unknown_tokens_with_context() {
        let t = TokenMap::new();
        let err = render("hi {__nope__}", &t, "src/a.ts").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope") && msg.contains("src/a.ts"), "{msg}");
    }

    #[test]
    fn render_leaves_non_token_text_alone() {
        let t = TokenMap::new();
        assert_eq!(render("a {__ b __} c", &t, "x").unwrap(), "a {__ b __} c");
        assert_eq!(render("open {__ never closed", &t, "x").unwrap(), "open {__ never closed");
        assert_eq!(render("{____}", &t, "x").unwrap(), "{____}");
    }

    #[test]
    fn guids_are_stable_within_a_scope_and_differ_between_scopes() {
        let mut src = SequentialGuids::new();
        let mut a = TokenMap::new();
        a.insert_guid("webpartId", &mut src).unwrap();
        let mut b = TokenMap::new();
        b.insert_guid("webpartId", &mut src).unwrap();

        let twice = render("{__webpartId__}|{__webpartId__}", &a, "x").unwrap();
        let (l, r) = twice.split_once('|').unwrap();
        assert_eq!(l, r);
        assert_ne!(a.get("webpartId"), b.get("webpartId"));
    }

    #[test]
    fn random_guids_look_like_guids() {
        let g = RandomGuids.next_guid();
        assert_eq!(g.len(), 36);
        assert_eq!(g, g.to_lowercase());
    }

    #[test]
    fn invalid_token_names_are_rejected() {
        let mut t = TokenMap::new();
        assert!(t.insert("bad name", "x").is_err());
        assert!(t.insert("", "x").is_err());
        assert!(t.insert("bad_name", "x").is_err());
    }
}
