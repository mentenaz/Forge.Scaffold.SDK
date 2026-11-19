//! Editing a JSON file that has comments, without losing them.
//!
//! SPFx config files are JSON with comments and trailing commas allowed (the
//! web part manifests have both). Parsing such a file with `serde_json` and writing
//! it back would drop every comment, so this module edits the text instead: it
//! finds where a new member or element belongs and splices it in. Nothing that is
//! already in the file is rewritten, so comments, indentation, line endings,
//! blank lines and trailing commas all stay exactly as they were.
//!
//! How it works: blanking the comments and the trailing commas with spaces gives a
//! valid JSON document of *exactly the same length*, so every position found in it
//! is a position in the original. The blanked copy is parsed for positions and for
//! a [`serde_json::Value`] view; the edits are made to the original.
//!
//! Only three edits exist: add a member to an object, push an element onto an
//! array, replace a value. They are all a merge needs. The new text copies the
//! style of what is there: an object or array written on one line gets an inline
//! entry, a multi-line one gets a new line indented like its last entry, and an
//! empty one is opened up. Not supported: single-quoted strings, unquoted keys
//! (JSON5): those are an error, not a guess.

use crate::manifest::MergeStrategy;
use crate::plan::detect_indent;
use serde_json::Value;

/// One step on the way to a value: an object key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seg {
    Key(String),
    Index(usize),
}

impl Seg {
    pub(crate) fn key(k: &str) -> Seg {
        Seg::Key(k.to_string())
    }
}

#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Scalar,
}

#[derive(Debug)]
struct Member {
    key: String,
    key_start: usize,
    value: Node,
}

/// A JSON-with-comments document being edited.
#[derive(Debug, Clone)]
pub(crate) struct Doc {
    text: String,
}

impl Doc {
    /// Checks that `text` is JSON with comments (and trailing commas).
    pub(crate) fn open(text: &str) -> Result<Doc, String> {
        let doc = Doc { text: text.to_string() };
        doc.parsed()?;
        Ok(doc)
    }

    pub(crate) fn into_text(self) -> String {
        self.text
    }

    /// The document as a value, comments ignored.
    pub(crate) fn value(&self) -> Result<Value, String> {
        let (blanked, _) = blank(&self.text)?;
        serde_json::from_str(&blanked).map_err(|e| e.to_string())
    }

    /// The positions in the document, where the trailing commas are, and the blanked copy.
    fn parsed(&self) -> Result<(Node, Vec<usize>, String), String> {
        let (blanked, commas) = blank(&self.text)?;
        serde_json::from_str::<Value>(&blanked).map_err(|e| format!("not valid JSON: {e}"))?;
        let mut scanner = Scanner { s: blanked.as_bytes(), i: 0, text: &blanked };
        scanner.ws();
        let root = scanner.value()?;
        Ok((root, commas, blanked))
    }

    /// Adds `key: value` at the end of the object at `path`. The key must not be there yet.
    pub(crate) fn add_member(&mut self, path: &[Seg], key: &str, value: &Value) -> Result<(), String> {
        let (root, commas, blanked) = self.parsed()?;
        let node = at(&root, path).ok_or_else(|| format!("there is nothing at {}", show(path)))?;
        let Kind::Object(members) = &node.kind else {
            return Err(format!("{} is not an object", show(path)));
        };
        if members.iter().any(|m| m.key == key) {
            return Err(format!("{} already has '{key}'", show(path)));
        }
        let key_json = serde_json::to_string(key).map_err(|e| e.to_string())?;
        let style = Style::of(&self.text, node, members.last().map(|m| (m.key_start, m.value.end)));
        let entry = |value_text: String| format!("{key_json}: {value_text}");
        self.insert_entry(node, &commas, &blanked, &style, value, entry);
        Ok(())
    }

    /// Adds `value` at the end of the array at `path`.
    pub(crate) fn push_element(&mut self, path: &[Seg], value: &Value) -> Result<(), String> {
        let (root, commas, blanked) = self.parsed()?;
        let node = at(&root, path).ok_or_else(|| format!("there is nothing at {}", show(path)))?;
        let Kind::Array(items) = &node.kind else {
            return Err(format!("{} is not an array", show(path)));
        };
        let style = Style::of(&self.text, node, items.last().map(|n| (n.start, n.end)));
        self.insert_entry(node, &commas, &blanked, &style, value, |v| v);
        Ok(())
    }

    /// Replaces the value at `path`. Comments inside the replaced value go with it.
    pub(crate) fn replace_value(&mut self, path: &[Seg], value: &Value) -> Result<(), String> {
        let (root, _, _) = self.parsed()?;
        let node = at(&root, path).ok_or_else(|| format!("there is nothing at {}", show(path)))?;
        let multiline = self.text[node.start..node.end].contains('\n') || value.is_object() || value.is_array();
        let base = line_indent(&self.text, node.start);
        let unit = detect_indent(&self.text);
        let text = if multiline { render_multiline(value, &self.text, &unit, &base) } else { compact(value) };
        self.text.replace_range(node.start..node.end, &text);
        Ok(())
    }

    /// Inserts one entry (a member or an element) into `node`, in the style of the entries there.
    fn insert_entry(
        &mut self,
        node: &Node,
        commas: &[usize],
        blanked: &str,
        style: &Style,
        value: &Value,
        entry: impl Fn(String) -> String,
    ) {
        let nl = newline(&self.text);
        let unit = detect_indent(&self.text);
        let (open, close) = (node.start, node.end - 1); // the positions of the brackets

        match style.last {
            Some((_, last_end)) => {
                // a trailing comma after the last entry means the file uses them: keep using them
                let trailing = commas.iter().copied().find(|&c| c >= last_end && c < close);
                if style.multiline {
                    let text = entry(render_multiline(value, &self.text, &unit, &style.item_indent));
                    let indent = &style.item_indent;
                    // If only whitespace and comments follow the last entry on its line, the new entry
                    // goes after them, on a line of its own, so a comment stays with the line it is on.
                    let after = trailing.map_or(last_end, |c| c + 1);
                    let line_end = blanked[after..].find('\n').map(|p| after + p);
                    match line_end.filter(|&le| blanked[after..le].trim().is_empty()) {
                        Some(le) => {
                            let at = if self.text[..le].ends_with('\r') { le - 1 } else { le };
                            match trailing {
                                Some(_) => self.text.insert_str(at, &format!("{nl}{indent}{text},")),
                                None => {
                                    self.text.insert_str(at, &format!("{nl}{indent}{text}"));
                                    self.text.insert(last_end, ',');
                                }
                            }
                        }
                        None => match trailing {
                            Some(c) => self.text.insert_str(c + 1, &format!("{nl}{indent}{text},")),
                            None => self.text.insert_str(last_end, &format!(",{nl}{indent}{text}")),
                        },
                    }
                } else {
                    let text = entry(compact(value));
                    match trailing {
                        Some(c) => self.text.insert_str(c + 1, &format!(" {text},")),
                        None => self.text.insert_str(last_end, &format!(", {text}")),
                    }
                }
            }
            None => {
                // empty: open it up
                let inner_indent = format!("{}{unit}", style.base_indent);
                let text = entry(render_multiline(value, &self.text, &unit, &inner_indent));
                let inner = &self.text[open + 1..close];
                if inner.trim().is_empty() {
                    self.text.replace_range(open + 1..close, &format!("{nl}{inner_indent}{text}{nl}{}", style.base_indent));
                } else {
                    // it holds a comment: put the entry first and leave the rest alone
                    self.text.insert_str(open + 1, &format!("{nl}{inner_indent}{text}"));
                }
            }
        }
    }
}

/// How the entries of one object or array are laid out.
struct Style {
    multiline: bool,
    /// The indent of the line the object or array starts on.
    base_indent: String,
    /// The indent a new entry gets (multi-line only).
    item_indent: String,
    /// Where the last entry starts and ends, if there is one.
    last: Option<(usize, usize)>,
}

impl Style {
    fn of(text: &str, node: &Node, last: Option<(usize, usize)>) -> Style {
        let multiline = text[node.start..node.end].contains('\n');
        let base_indent = line_indent(text, node.start);
        let item_indent = match last {
            Some((start, _)) if text[node.start..start].contains('\n') => line_indent(text, start),
            _ => format!("{base_indent}{}", detect_indent(text)),
        };
        Style { multiline, base_indent, item_indent, last }
    }
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn line_indent(text: &str, offset: usize) -> String {
    let start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    text[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("a Value always serializes")
}

/// `value` as pretty JSON in the file's own indent, every line after the first indented by `indent`.
fn render_multiline(value: &Value, text: &str, unit: &str, indent: &str) -> String {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(value, &mut ser).expect("a Value always serializes");
    String::from_utf8(buf)
        .expect("JSON is UTF-8")
        .replace('\n', &format!("{}{indent}", newline(text)))
}

fn show(path: &[Seg]) -> String {
    if path.is_empty() {
        return "the top level".into();
    }
    path.iter()
        .map(|s| match s {
            Seg::Key(k) => k.clone(),
            Seg::Index(i) => format!("[{i}]"),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn at<'a>(root: &'a Node, path: &[Seg]) -> Option<&'a Node> {
    let mut node = root;
    for seg in path {
        node = match (seg, &node.kind) {
            (Seg::Key(k), Kind::Object(members)) => &members.iter().find(|m| &m.key == k)?.value,
            (Seg::Index(i), Kind::Array(items)) => items.get(*i)?,
            _ => return None,
        };
    }
    Some(node)
}

// ------------------------------------------------------------------ blanking

/// Replaces comments and trailing commas with spaces (newlines are kept), so the
/// result is valid JSON of the same length. Also returns where the trailing commas were.
fn blank(text: &str) -> Result<(String, Vec<usize>), String> {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                b'\n' => return Err("a string runs past the end of its line".into()),
                _ => {}
            }
            i += 1;
            continue;
        }
        match (c, b.get(i + 1)) {
            (b'"', _) => {
                in_string = true;
                i += 1;
            }
            (b'/', Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    if b[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                loop {
                    if i + 1 >= b.len() {
                        return Err("a /* comment is never closed".into());
                    }
                    if b[i] == b'*' && b[i + 1] == b'/' {
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                        break;
                    }
                    if b[i] != b'\n' && b[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            (b'\'', _) => return Err("single-quoted strings are not supported".into()),
            _ => i += 1,
        }
    }
    if in_string {
        return Err("a string is never closed".into());
    }

    // trailing commas: a comma followed only by whitespace and then } or ]
    let mut commas = Vec::new();
    let (mut in_string, mut i) = (false, 0);
    while i < out.len() {
        match out[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b',' if !in_string => {
                let next = out[i + 1..].iter().find(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'));
                if matches!(next, Some(b'}' | b']')) {
                    commas.push(i);
                    out[i] = b' ';
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok((String::from_utf8(out).expect("only ASCII bytes were replaced by spaces"), commas))
}

// ------------------------------------------------------------------ scanning

/// Reads positions out of a document that is already known to be valid JSON.
struct Scanner<'a> {
    s: &'a [u8],
    i: usize,
    text: &'a str,
}

impl Scanner<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn string_end(&mut self) -> usize {
        self.i += 1; // the opening quote
        while self.s[self.i] != b'"' {
            if self.s[self.i] == b'\\' {
                self.i += 1;
            }
            self.i += 1;
        }
        self.i += 1;
        self.i
    }

    fn value(&mut self) -> Result<Node, String> {
        let start = self.i;
        let kind = match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                loop {
                    self.ws();
                    if self.s[self.i] == b'}' {
                        self.i += 1;
                        break;
                    }
                    let key_start = self.i;
                    let key_end = self.string_end();
                    let key: String = serde_json::from_str(&self.text[key_start..key_end]).map_err(|e| e.to_string())?;
                    self.ws();
                    self.i += 1; // the colon
                    self.ws();
                    let value = self.value()?;
                    members.push(Member { key, key_start, value });
                    self.ws();
                    if self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
                Kind::Object(members)
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.s[self.i] == b']' {
                        self.i += 1;
                        break;
                    }
                    items.push(self.value()?);
                    self.ws();
                    if self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
                Kind::Array(items)
            }
            Some(b'"') => {
                self.string_end();
                Kind::Scalar
            }
            Some(_) => {
                while self.i < self.s.len() && !matches!(self.s[self.i], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                    self.i += 1;
                }
                Kind::Scalar
            }
            None => return Err("the document ends too soon".into()),
        };
        Ok(Node { start, end: self.i, kind })
    }
}

// ----------------------------------------------------------- merge by path

/// The dotted-path `json-set` / `json-append` of a generator's `merge`, done on text so
/// that comments survive. Missing parents are created, like the whole-document version.
pub(crate) fn merge_path(text: &str, segments: &[&str], strategy: MergeStrategy, value: &Value) -> Result<String, String> {
    let mut doc = Doc::open(text)?;
    let current = doc.value()?;
    let (last, parents) = segments.split_last().ok_or("the path is empty")?;

    // follow the path as far as it exists
    let mut path: Vec<Seg> = Vec::new();
    let mut node = &current;
    let mut depth = 0;
    for seg in parents {
        match node.as_object().ok_or_else(|| format!("'{seg}' is not inside an object"))?.get(*seg) {
            Some(next) => {
                path.push(Seg::key(seg));
                node = next;
                depth += 1;
            }
            None => break,
        }
    }

    // the first missing parent: create it with everything below it
    if depth < parents.len() {
        node.as_object().ok_or_else(|| format!("the parent of '{}' is not an object", parents[depth]))?;
        let mut nested = match strategy {
            MergeStrategy::JsonSet => value.clone(),
            MergeStrategy::JsonAppend => Value::Array(vec![value.clone()]),
        };
        for seg in segments[depth + 1..].iter().rev() {
            let mut map = serde_json::Map::new();
            map.insert((*seg).to_string(), nested);
            nested = Value::Object(map);
        }
        doc.add_member(&path, segments[depth], &nested)?;
        return Ok(doc.into_text());
    }

    let parent = node.as_object().ok_or_else(|| format!("the parent of '{last}' is not an object"))?;
    match (parent.get(*last), strategy) {
        (None, MergeStrategy::JsonSet) => doc.add_member(&path, last, value)?,
        (None, MergeStrategy::JsonAppend) => doc.add_member(&path, last, &Value::Array(vec![value.clone()]))?,
        (Some(_), MergeStrategy::JsonSet) => {
            path.push(Seg::key(last));
            doc.replace_value(&path, value)?;
        }
        (Some(Value::Array(_)), MergeStrategy::JsonAppend) => {
            path.push(Seg::key(last));
            doc.push_element(&path, value)?;
        }
        (Some(_), MergeStrategy::JsonAppend) => return Err("it exists but is not an array".into()),
    }
    Ok(doc.into_text())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn add(text: &str, path: &[Seg], key: &str, value: Value) -> String {
        let mut d = Doc::open(text).unwrap();
        d.add_member(path, key, &value).unwrap();
        d.into_text()
    }

    fn k(s: &str) -> Seg {
        Seg::key(s)
    }

    #[test]
    fn blanking_keeps_the_length_and_finds_trailing_commas() {
        let text = "{ // note \u{e9}\n  \"a\": \"// not a comment\", /* two\nlines */ \"b\": [1, 2,],\n}";
        let (blanked, commas) = blank(text).unwrap();
        assert_eq!(blanked.len(), text.len());
        assert!(blanked.contains("\"// not a comment\""), "strings are left alone");
        assert!(!blanked.contains("note") && !blanked.contains("lines"));
        assert_eq!(blanked.matches('\n').count(), text.matches('\n').count(), "line breaks stay");
        assert_eq!(commas.len(), 2, "the comma after [1, 2 and the one after the last member");
        let v: Value = serde_json::from_str(&blanked).unwrap();
        assert_eq!(v, json!({ "a": "// not a comment", "b": [1, 2] }));
    }

    #[test]
    fn what_cannot_be_read_is_an_error() {
        for bad in ["{ /* never closed", "{ \"a\": \"open }", "{ 'a': 1 }", "{ \"a\": }", "{ a: 1 }", "[1, 2", ""] {
            assert!(Doc::open(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_member_is_added_after_the_last_one_and_every_comment_stays() {
        let text = "{\n  // the bundles\n  \"bundles\": {\n    \"a\": 1 // first\n  }, /* end */\n  \"x\": 2\n}\n";
        let out = add(text, &[k("bundles")], "b", json!({ "components": [1] }));
        assert_eq!(
            out,
            "{\n  // the bundles\n  \"bundles\": {\n    \"a\": 1, // first\n    \"b\": {\n      \"components\": [\n        1\n      ]\n    }\n  }, /* end */\n  \"x\": 2\n}\n"
        );
        // the comment after "a" moved with its line, and the result still reads as JSON
        assert_eq!(Doc::open(&out).unwrap().value().unwrap()["bundles"]["b"]["components"], json!([1]));
    }

    #[test]
    fn indent_tabs_and_crlf_are_copied() {
        let tabs = "{\n\t\"a\": 1\n}";
        assert_eq!(add(tabs, &[], "b", json!(2)), "{\n\t\"a\": 1,\n\t\"b\": 2\n}");
        let crlf = "{\r\n    \"a\": 1\r\n}\r\n";
        assert_eq!(add(crlf, &[], "b", json!({ "c": 3 })), "{\r\n    \"a\": 1,\r\n    \"b\": {\r\n        \"c\": 3\r\n    }\r\n}\r\n");
    }

    #[test]
    fn a_file_that_uses_trailing_commas_keeps_using_them() {
        let text = "{\n  \"a\": 1,\n}\n";
        assert_eq!(add(text, &[], "b", json!(2)), "{\n  \"a\": 1,\n  \"b\": 2,\n}\n");
        assert_eq!(add("{ \"a\": 1, }", &[], "b", json!(2)), "{ \"a\": 1, \"b\": 2, }");
    }

    #[test]
    fn an_object_on_one_line_gets_an_inline_member() {
        assert_eq!(add("{ \"a\": 1 }", &[], "b", json!({ "c": [1] })), "{ \"a\": 1, \"b\": {\"c\":[1]} }");
    }

    #[test]
    fn an_empty_object_is_opened_up_and_one_with_only_a_comment_keeps_it() {
        assert_eq!(add("{\n  \"m\": {}\n}\n", &[k("m")], "a", json!(1)), "{\n  \"m\": {\n    \"a\": 1\n  }\n}\n");
        assert_eq!(add("{}", &[], "a", json!(1)), "{\n  \"a\": 1\n}");
        assert_eq!(
            add("{\n  \"m\": { // nothing yet\n  }\n}\n", &[k("m")], "a", json!(1)),
            "{\n  \"m\": {\n    \"a\": 1 // nothing yet\n  }\n}\n"
        );
    }

    #[test]
    fn elements_are_pushed_inline_multi_line_and_into_empty_arrays() {
        let push = |text: &str, path: &[Seg], v: Value| {
            let mut d = Doc::open(text).unwrap();
            d.push_element(path, &v).unwrap();
            d.into_text()
        };
        assert_eq!(push("{ \"a\": [\"x\", \"y\"] }", &[k("a")], json!("z")), "{ \"a\": [\"x\", \"y\", \"z\"] }");
        assert_eq!(
            push("{\n  \"a\": [\n    { \"id\": 1 }\n  ]\n}\n", &[k("a")], json!({ "id": 2 })),
            "{\n  \"a\": [\n    { \"id\": 1 },\n    {\n      \"id\": 2\n    }\n  ]\n}\n"
        );
        assert_eq!(push("{\n  \"a\": []\n}\n", &[k("a")], json!("x")), "{\n  \"a\": [\n    \"x\"\n  ]\n}\n");
        // an array inside an array element, by index
        assert_eq!(
            push("{ \"f\": [ { \"m\": [\"a\"] }, { \"m\": [] } ] }", &[k("f"), Seg::Index(0), k("m")], json!("b")),
            "{ \"f\": [ { \"m\": [\"a\", \"b\"] }, { \"m\": [] } ] }"
        );
    }

    #[test]
    fn a_value_is_replaced_in_place() {
        let mut d = Doc::open("{\n  \"a\": 1, // keep me\n  \"b\": \"old\"\n}\n").unwrap();
        d.replace_value(&[k("b")], &json!("new")).unwrap();
        assert_eq!(d.into_text(), "{\n  \"a\": 1, // keep me\n  \"b\": \"new\"\n}\n");
    }

    #[test]
    fn wrong_targets_are_errors() {
        let mut d = Doc::open("{ \"a\": 1, \"l\": [] }").unwrap();
        assert!(d.add_member(&[k("a")], "x", &json!(1)).is_err(), "a number is not an object");
        assert!(d.add_member(&[k("nope")], "x", &json!(1)).is_err());
        assert!(d.add_member(&[], "a", &json!(1)).is_err(), "the key exists");
        assert!(d.push_element(&[k("a")], &json!(1)).is_err());
        assert!(d.push_element(&[k("l"), Seg::Index(3)], &json!(1)).is_err());
    }

    #[test]
    fn merge_by_dotted_path_on_text_keeps_comments_and_creates_parents() {
        let text = "{\n  // SPFx config\n  \"version\": \"2.0\",\n  \"bundles\": {\n    \"a-web-part\": {} // existing\n  }\n}\n";
        let set = merge_path(text, &["bundles", "b-web-part"], MergeStrategy::JsonSet, &json!({ "components": [] })).unwrap();
        assert!(set.contains("// SPFx config") && set.contains("// existing"));
        let v = Doc::open(&set).unwrap().value().unwrap();
        assert!(v["bundles"]["a-web-part"].is_object() && v["bundles"]["b-web-part"]["components"] == json!([]));

        // missing parents are created, all the way down
        let deep = merge_path("{ // c\n}", &["localizedResources", "X", "y"], MergeStrategy::JsonSet, &json!("v")).unwrap();
        assert_eq!(Doc::open(&deep).unwrap().value().unwrap(), json!({ "localizedResources": { "X": { "y": "v" } } }));
        assert!(deep.contains("// c"));

        // json-set on an existing key replaces, json-append pushes
        let replaced = merge_path("{ \"a\": 1 }", &["a"], MergeStrategy::JsonSet, &json!(2)).unwrap();
        assert_eq!(replaced, "{ \"a\": 2 }");
        let appended = merge_path("{ \"l\": [1] } // end", &["l"], MergeStrategy::JsonAppend, &json!(2)).unwrap();
        assert_eq!(appended, "{ \"l\": [1, 2] } // end");
        let created = merge_path("{ \"x\": 1 }", &["l"], MergeStrategy::JsonAppend, &json!(1)).unwrap();
        assert_eq!(Doc::open(&created).unwrap().value().unwrap()["l"], json!([1]));
        assert!(merge_path("{ \"l\": 5 }", &["l"], MergeStrategy::JsonAppend, &json!(1)).is_err());
        assert!(merge_path("{ \"l\": 5 }", &["l", "x"], MergeStrategy::JsonSet, &json!(1)).is_err());
    }

    #[test]
    fn a_comment_on_the_last_entrys_line_stays_with_it_with_crlf_and_when_the_brace_shares_the_line() {
        assert_eq!(
            add("{\r\n  \"a\": 1 // c\r\n}\r\n", &[], "b", json!(2)),
            "{\r\n  \"a\": 1, // c\r\n  \"b\": 2\r\n}\r\n"
        );
        // the closing brace on the last entry's line: nothing can go after it, so the entry goes straight after
        assert_eq!(add("{\n  \"a\": 1 }", &[], "b", json!(2)), "{\n  \"a\": 1,\n  \"b\": 2 }");
        // with a trailing comma and a comment
        assert_eq!(
            add("{\n  \"a\": 1, // c\n}\n", &[], "b", json!(2)),
            "{\n  \"a\": 1, // c\n  \"b\": 2,\n}\n"
        );
    }
}
