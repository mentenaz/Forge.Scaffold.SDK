//! Input validation for the values the user types in the panel.
//!
//! These return *all* problems at once (as data) so the panel can show them
//! next to the field as the user types, instead of failing one at a time.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Which input the issue belongs to, e.g. `solutionName` or `webparts[1]`.
    pub field: String,
    pub message: String,
}

impl ValidationIssue {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

const MAX_SOLUTION_NAME: usize = 50;
const MAX_WEBPART_NAME: usize = 40;

/// Names Windows refuses as file or folder names (any case, with or without extension).
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Words that cannot be used as identifiers in (strict-mode) TypeScript.
const RESERVED_WORDS: &[&str] = &[
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue",
    "debugger", "default", "delete", "do", "else", "enum", "export", "extends", "false",
    "finally", "for", "function", "if", "implements", "import", "in", "instanceof", "interface",
    "let", "new", "null", "package", "private", "protected", "public", "return", "static",
    "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with",
    "yield",
];

fn is_windows_reserved(name: &str) -> bool {
    WINDOWS_RESERVED.contains(&name.to_lowercase().as_str())
}

/// The solution name becomes the folder name *and* the npm package name, so it
/// must be valid for both: lowercase letters, digits and single hyphens,
/// starting with a letter.
pub fn validate_solution_name(name: &str) -> Vec<ValidationIssue> {
    const F: &str = "solutionName";
    let mut issues = Vec::new();

    if name.is_empty() {
        issues.push(ValidationIssue::new(F, "The solution name is required"));
        return issues;
    }
    if name.chars().count() > MAX_SOLUTION_NAME {
        issues.push(ValidationIssue::new(
            F,
            format!("Use at most {MAX_SOLUTION_NAME} characters (long paths break npm on Windows)"),
        ));
    }
    if name.chars().any(char::is_whitespace) {
        issues.push(ValidationIssue::new(F, "No spaces: use '-' instead"));
    }
    if name.chars().any(|c| c.is_uppercase()) {
        issues.push(ValidationIssue::new(F, "Use lowercase letters only (npm package names must be lowercase)"));
    }

    let mut bad: Vec<char> = name
        .chars()
        .filter(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
        .filter(|c| !c.is_whitespace() && !c.is_uppercase())
        .collect();
    bad.sort_unstable();
    bad.dedup();
    if !bad.is_empty() {
        let list: Vec<String> = bad.iter().map(|c| format!("'{c}'")).collect();
        issues.push(ValidationIssue::new(
            F,
            format!("Not allowed: {}. Use letters, digits and '-' only", list.join(" ")),
        ));
    }

    if let Some(first) = name.chars().next() {
        if !first.is_alphabetic() {
            issues.push(ValidationIssue::new(F, "Must start with a letter"));
        }
    }
    if name.ends_with('-') {
        issues.push(ValidationIssue::new(F, "Must not end with '-'"));
    }
    if name.contains("--") {
        issues.push(ValidationIssue::new(F, "Must not contain '--'"));
    }
    if is_windows_reserved(name) {
        issues.push(ValidationIssue::new(F, "This name is reserved by Windows"));
    }
    issues
}

/// Web part names become class names, manifest aliases and folder names.
/// Letters and digits only, starting with a letter, unique ignoring case.
pub fn validate_webpart_names(names: &[String]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if names.is_empty() {
        issues.push(ValidationIssue::new("webparts", "Add at least one web part"));
        return issues;
    }

    let mut seen: HashSet<String> = HashSet::new();
    for (i, name) in names.iter().enumerate() {
        let field = format!("webparts[{i}]");

        if name.is_empty() {
            issues.push(ValidationIssue::new(&field, "The web part name is required"));
            continue;
        }
        if name.chars().count() > MAX_WEBPART_NAME {
            issues.push(ValidationIssue::new(
                &field,
                format!("Use at most {MAX_WEBPART_NAME} characters"),
            ));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            issues.push(ValidationIssue::new(
                &field,
                "Use letters and digits only (no spaces or symbols)",
            ));
        }
        if let Some(first) = name.chars().next() {
            if !first.is_ascii_alphabetic() {
                issues.push(ValidationIssue::new(&field, "Must start with a letter"));
            }
        }
        let lower = name.to_lowercase();
        if RESERVED_WORDS.contains(&lower.as_str()) {
            issues.push(ValidationIssue::new(
                &field,
                format!("'{name}' is a reserved word and cannot be used"),
            ));
        }
        if is_windows_reserved(name) {
            issues.push(ValidationIssue::new(&field, "This name is reserved by Windows"));
        }
        if !seen.insert(lower) {
            issues.push(ValidationIssue::new(
                &field,
                format!("'{name}' is used more than once (names are compared ignoring case)"),
            ));
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(issues: &[ValidationIssue], needle: &str) -> bool {
        issues.iter().any(|i| i.message.contains(needle))
    }

    #[test]
    fn good_solution_names_pass() {
        for ok in ["my-solution", "sol1", "a", "hello-world-2"] {
            assert!(validate_solution_name(ok).is_empty(), "{ok}");
        }
    }

    #[test]
    fn bad_solution_names_explain_why() {
        assert!(has(&validate_solution_name(""), "required"));
        assert!(has(&validate_solution_name("My Solution"), "No spaces"));
        assert!(has(&validate_solution_name("My Solution"), "lowercase"));
        assert!(has(&validate_solution_name("1abc"), "start with a letter"));
        assert!(has(&validate_solution_name("abc-"), "end with"));
        assert!(has(&validate_solution_name("a--b"), "'--'"));
        assert!(has(&validate_solution_name("a/b"), "'/'"));
        assert!(has(&validate_solution_name("con"), "Windows"));
        assert!(has(&validate_solution_name(&"a".repeat(51)), "at most"));
    }

    #[test]
    fn good_webpart_names_pass() {
        let names = vec!["HelloWorld".to_string(), "Chart2".to_string()];
        assert!(validate_webpart_names(&names).is_empty());
    }

    #[test]
    fn bad_webpart_names_are_reported_per_field() {
        let names: Vec<String> = ["Hello World", "2fast", "Class", "helloworld", "HelloWorld", "NUL", ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let issues = validate_webpart_names(&names);

        let for_field = |f: &str| issues.iter().filter(|i| i.field == f).count();
        assert!(for_field("webparts[0]") >= 1); // space
        assert!(has(&issues, "start with a letter")); // 2fast
        assert!(has(&issues, "reserved word")); // Class
        assert!(has(&issues, "more than once")); // helloworld vs HelloWorld (or reverse)
        assert!(has(&issues, "Windows")); // NUL
        assert!(has(&issues, "required")); // empty
    }

    #[test]
    fn empty_webpart_list_is_an_issue() {
        assert!(has(&validate_webpart_names(&[]), "at least one"));
    }
}
