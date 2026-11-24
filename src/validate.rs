//! Input validation for the values the user types in the panel.
//!
//! These return *all* problems at once (as data) so the panel can show them
//! next to the field as the user types, instead of failing one at a time.

use std::collections::HashSet;

/// One problem with an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Which input the issue belongs to, e.g. `solutionName` or `webparts[1]`.
    pub field: String,
    /// What is wrong, written for the user.
    pub message: String,
}

impl ValidationIssue {
    pub(crate) fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

const MAX_SOLUTION_NAME: usize = 50;
const MAX_PACKAGE_NAME: usize = 214;
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
///
/// # Example
///
/// ```
/// use forge_scaffold::validate_solution_name;
///
/// assert!(validate_solution_name("intranet-home").is_empty());
/// // uppercase letters and spaces are not allowed: the name is also a folder and a package name
/// assert!(!validate_solution_name("Intranet Home").is_empty());
/// ```
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
        if lower.ends_with("webpart") {
            issues.push(ValidationIssue::new(
                &field,
                "Leave out 'WebPart' at the end: it is added for you",
            ));
        }
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

/// Names npm refuses for a package.
const NPM_BLOCKED: &[&str] = &["node_modules", "favicon.ico"];

/// An npm package name, the way `create-vite` checks one (plus npm's own length limit): lowercase letters,
/// digits and `- . _ ~`, not starting with `.` or `_`, optionally scoped (`@scope/name`). Unlike a solution
/// name this may keep a dot or an underscore and may start with a digit. Use it for a text prompt with
/// `"rule": "package-name"`.
///
/// A name that is also the folder name (`folderFrom`) must not be scoped: the folder has to be one name.
pub fn validate_package_name(name: &str) -> Vec<ValidationIssue> {
    const F: &str = "packageName";
    let mut issues = Vec::new();

    if name.is_empty() {
        issues.push(ValidationIssue::new(F, "The package name is required"));
        return issues;
    }
    if name.chars().count() > MAX_PACKAGE_NAME {
        issues.push(ValidationIssue::new(F, format!("Use at most {MAX_PACKAGE_NAME} characters")));
    }
    if name.chars().any(char::is_whitespace) {
        issues.push(ValidationIssue::new(F, "No spaces: use '-' instead"));
    }
    if name.chars().any(|c| c.is_uppercase()) {
        issues.push(ValidationIssue::new(F, "Use lowercase letters only (npm package names must be lowercase)"));
    }

    let (scope, bare) = match name.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, bare)) => (Some(scope), bare),
            None => {
                issues.push(ValidationIssue::new(F, "A scoped name looks like '@scope/name'"));
                return issues;
            }
        },
        None => (None, name),
    };
    if bare.contains('/') || bare.contains('@') {
        issues.push(ValidationIssue::new(F, "'@' and '/' are only allowed in a scope, as '@scope/name'"));
        return issues;
    }

    let mut bad: Vec<char> = scope
        .into_iter()
        .chain(Some(bare))
        .flat_map(str::chars)
        .filter(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_' | '~')))
        .filter(|c| !c.is_whitespace() && !c.is_uppercase())
        .collect();
    bad.sort_unstable();
    bad.dedup();
    if !bad.is_empty() {
        let list: Vec<String> = bad.iter().map(|c| format!("'{c}'")).collect();
        issues.push(ValidationIssue::new(
            F,
            format!("Not allowed: {}. Use letters, digits and '- . _ ~' only", list.join(" ")),
        ));
    }
    for (label, part) in scope.map(|s| ("The scope", s)).into_iter().chain(Some(("The name", bare))) {
        if part.is_empty() {
            issues.push(ValidationIssue::new(F, format!("{label} must not be empty")));
        } else if part.starts_with(['.', '_']) {
            issues.push(ValidationIssue::new(F, format!("{label} must not start with '.' or '_'")));
        }
    }
    if NPM_BLOCKED.contains(&name) {
        issues.push(ValidationIssue::new(F, format!("'{name}' is not allowed as a package name")));
    }
    issues
}

/// A valid package name made from whatever the user typed, for pre-filling a field. It is the fix-up
/// `create-vite` does: trimmed and lowercase, every run of whitespace becomes one '-', one leading '.' or
/// '_' is dropped, and every run of anything outside `a-z 0-9 - ~` becomes one '-'. A scope is not kept
/// (the `@` and `/` become '-'). The result can be empty.
///
/// # Example
///
/// ```
/// use forge_scaffold::{suggest_package_name, validate_package_name};
///
/// let name = suggest_package_name("  My  Cool App ");
/// assert_eq!(name, "my-cool-app");
/// assert!(validate_package_name(&name).is_empty());
/// ```
pub fn suggest_package_name(input: &str) -> String {
    let lowered = input.trim().to_lowercase();

    let mut spaced = String::new();
    let mut in_space = false;
    for c in lowered.chars() {
        if c.is_whitespace() {
            if !in_space {
                spaced.push('-');
            }
            in_space = true;
        } else {
            spaced.push(c);
            in_space = false;
        }
    }

    let body = spaced.strip_prefix(['.', '_']).unwrap_or(&spaced);
    let mut out = String::new();
    let mut in_bad = false;
    for c in body.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '~' {
            out.push(c);
            in_bad = false;
        } else {
            if !in_bad {
                out.push('-');
            }
            in_bad = true;
        }
    }
    out
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
    fn names_ending_in_webpart_are_rejected() {
        let names = vec!["HelloWorldWebPart".to_string(), "Chartwebpart".to_string()];
        let issues = validate_webpart_names(&names);
        assert_eq!(issues.iter().filter(|i| i.message.contains("Leave out")).count(), 2);
        // a name that only contains the word is fine
        assert!(validate_webpart_names(&["WebPartList".to_string()]).is_empty());
    }

    #[test]
    fn empty_webpart_list_is_an_issue() {
        assert!(has(&validate_webpart_names(&[]), "at least one"));
    }

    // ---------------------------------------------------------- package names

    #[test]
    fn good_package_names_pass() {
        for ok in ["vite-app", "app", "my.app", "a_b", "123", "@scope/app", "~tilde", "a-b.c_d~e", "@my-org/ui.kit"] {
            assert!(validate_package_name(ok).is_empty(), "{ok}: {:?}", validate_package_name(ok));
        }
    }

    #[test]
    fn bad_package_names_say_why() {
        let bad = |name: &str| validate_package_name(name);
        assert!(has(&bad(""), "required"));
        assert!(has(&bad("My App"), "No spaces") && has(&bad("My App"), "lowercase"));
        assert!(has(&bad("my app"), "No spaces"));
        assert!(has(&bad("Upper"), "lowercase"));
        assert!(has(&bad("a!b"), "'!'"));
        assert!(has(&bad(".hidden"), "must not start with '.' or '_'"));
        assert!(has(&bad("_private"), "must not start with '.' or '_'"));
        assert!(has(&bad("@scope"), "@scope/name"));
        assert!(has(&bad("@scope/"), "must not be empty"));
        assert!(has(&bad("@/app"), "must not be empty"));
        assert!(has(&bad("@scope/a/b"), "only allowed in a scope"));
        assert!(has(&bad("a/b"), "only allowed in a scope"));
        assert!(has(&bad("node_modules"), "not allowed as a package name"));
        assert!(has(&bad(&"a".repeat(215)), "at most 214"));
        // 214 is fine
        assert!(validate_package_name(&"a".repeat(214)).is_empty());
        // the field names the generic input, which a prompt rule points at its own key
        assert_eq!(bad("A")[0].field, "packageName");
    }

    #[test]
    fn a_valid_package_name_is_suggested_the_way_create_vite_does() {
        for (input, want) in [
            ("My App", "my-app"),
            ("  Hello   World ", "hello-world"),
            ("_private", "private"),
            (".hidden", "hidden"),
            ("a b!c", "a-b-c"),
            ("!!a", "-a"),
            ("a- b", "a--b"),
            ("@org/app", "-org-app"),
            ("already-fine", "already-fine"),
            ("", ""),
        ] {
            assert_eq!(suggest_package_name(input), want, "{input:?}");
        }
        // what it suggests is valid, unless it is empty or starts with '-' (which is valid for create-vite too)
        for input in ["My App", "Hello World", "a b!c"] {
            assert!(validate_package_name(&suggest_package_name(input)).is_empty(), "{input}");
        }
    }
}
