//! The template index and the two small state files kept next to the templates.
//!
//! * [`RemoteIndex`]: the `index.json` published next to the template archives.
//! * [`CachedIndex`]: our local copy plus the ETag and the time of the last check.
//! * [`InstalledFile`]: which template folders *we* downloaded (everything else
//!   in the templates folder belongs to the user and is never touched).

use crate::error::{Result, ScaffoldError};
use crate::stage::normalize_rel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashSet;

/// The index format this crate writes. Schema 2 adds the optional `title`, `description` and `tags` of an entry.
pub const INDEX_SCHEMA: u32 = 2;
/// The oldest index format this crate still reads (schema 1 has no display fields).
pub const MIN_INDEX_SCHEMA: u32 = 1;
/// The format of `installed.json`, which is a local file and changes on its own.
pub(crate) const INSTALLED_SCHEMA: u32 = 1;
/// Upper bound for an index download.
pub(crate) const MAX_INDEX_BYTES: usize = 1024 * 1024;
/// Upper bound for one template archive.
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 25 * 1024 * 1024;

/// What the published `index.json` looks like:
///
/// ```json
/// { "schema": 1, "templates": [
///   { "generator": "spfx-webpart", "version": "1.23.2", "revision": 1,
///     "dir": "spfxv.1.23.2", "url": "https://…/spfxv.1.23.2.tar.gz",
///     "sha256": "<64 hex>", "size": 73210 } ] }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteIndex {
    /// The format number: 1 or 2 (see [`INDEX_SCHEMA`]).
    pub schema: u32,
    /// One entry per archive.
    pub templates: Vec<IndexEntry>,
}

/// One archive in the index: a generator, a version and a revision, where to download it and how to check it and, in
/// schema 2, what a list shows for it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexEntry {
    /// Generator id, equal to `id` in the archive's `manifest.json`.
    pub generator: String,
    /// Version of the thing being scaffolded (for SPFx: the SPFx version).
    pub version: String,
    /// Bumped whenever the templates for this version are fixed or improved.
    pub revision: u32,
    /// Folder name inside the templates folder, e.g. `spfxv.1.23.2`.
    pub dir: String,
    /// `https` URL of the `.tar.gz`.
    pub url: String,
    /// Lower-case hex SHA-256 of the archive.
    pub sha256: String,
    /// Size of the archive in bytes.
    pub size: u64,
    /// The name a list shows (schema 2). The id is shown when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// One sentence for a list (schema 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Words a list can filter by (schema 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl IndexEntry {
    /// Whether the entry carries any of the display fields.
    pub fn has_display(&self) -> bool {
        self.title.is_some() || self.description.is_some() || !self.tags.is_empty()
    }
}

impl RemoteIndex {
    /// Parses and validates an index. All problems are reported together.
    ///
    /// # Example
    ///
    /// ```
    /// use forge_scaffold::RemoteIndex;
    ///
    /// let json = format!(
    ///     r#"{{ "schema": 2, "templates": [ {{
    ///         "generator": "backend-api", "version": "1.0.0", "revision": 2, "dir": "backend-api.1.0.0",
    ///         "url": "https://example.com/backend-api.1.0.0.tar.gz", "sha256": "{}", "size": 2488,
    ///         "title": "Node API (Fastify)", "description": "A Node and TypeScript API.", "tags": ["node", "api"]
    ///     }} ] }}"#,
    ///     "a".repeat(64)
    /// );
    /// let index = RemoteIndex::from_json(json.as_bytes())?;
    /// let entry = index.find("backend-api", "1.0.0").expect("listed");
    /// assert_eq!(entry.title.as_deref(), Some("Node API (Fastify)"));
    /// assert_eq!(entry.tags, ["node", "api"]);
    /// // a format this version does not read is refused, not guessed at
    /// assert!(RemoteIndex::from_json(br#"{ "schema": 9, "templates": [] }"#).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_json(bytes: &[u8]) -> Result<RemoteIndex> {
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(ScaffoldError::Index(format!(
                "the index is larger than {MAX_INDEX_BYTES} bytes"
            )));
        }
        let index: RemoteIndex =
            serde_json::from_slice(bytes).map_err(|e| ScaffoldError::Index(e.to_string()))?;
        index.validate()?;
        Ok(index)
    }

    /// Checks the index and reports every problem together. [`RemoteIndex::from_json`] already does this.
    pub fn validate(&self) -> Result<()> {
        let mut issues = Vec::new();
        if !(MIN_INDEX_SCHEMA..=INDEX_SCHEMA).contains(&self.schema) {
            issues.push(format!(
                "schema {} is not supported (this version reads {MIN_INDEX_SCHEMA} to {INDEX_SCHEMA})",
                self.schema
            ));
        }
        let mut versions = HashSet::new();
        let mut dirs = HashSet::new();
        for e in &self.templates {
            let who = format!("{} {}", e.generator, e.version);
            if e.generator.is_empty()
                || !e
                    .generator
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                issues.push(format!("{who}: generator must be lowercase letters, digits and '-'"));
            }
            if parse_triplet(&e.version).is_none() {
                issues.push(format!("{who}: version must look like 1.23.2"));
            }
            if e.revision == 0 {
                issues.push(format!("{who}: revision starts at 1"));
            }
            match normalize_rel(&e.dir) {
                Ok(d) if d == e.dir && !d.contains('/') && !d.starts_with('.') => {}
                _ => issues.push(format!("{who}: dir '{}' must be one plain folder name", e.dir)),
            }
            if !e.url.starts_with("https://") {
                issues.push(format!("{who}: url must start with https://"));
            }
            if !is_sha256_hex(&e.sha256) {
                issues.push(format!("{who}: sha256 must be 64 lower-case hex characters"));
            }
            if e.size == 0 || e.size > MAX_ARCHIVE_BYTES {
                issues.push(format!("{who}: size must be between 1 and {MAX_ARCHIVE_BYTES} bytes"));
            }
            if self.schema < 2 && e.has_display() {
                issues.push(format!("{who}: title, description and tags need schema 2"));
            }
            for (_, problem) in crate::manifest::display_problems(e.title.as_deref(), e.description.as_deref(), &e.tags) {
                issues.push(format!("{who}: {problem}"));
            }
            if !versions.insert((e.generator.as_str(), e.version.as_str())) {
                issues.push(format!("{who} is listed twice"));
            }
            if !dirs.insert(e.dir.to_lowercase()) {
                issues.push(format!("dir '{}' is used twice", e.dir));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ScaffoldError::Index(issues.join("; ")))
        }
    }

    /// Entries of one generator, newest version first.
    pub fn for_generator(&self, generator: &str) -> Vec<&IndexEntry> {
        let mut found: Vec<&IndexEntry> =
            self.templates.iter().filter(|e| e.generator == generator).collect();
        found.sort_by(|a, b| compare_versions(&b.version, &a.version));
        found
    }

    /// The entry for one version of one generator.
    pub fn find(&self, generator: &str, version: &str) -> Option<&IndexEntry> {
        self.templates
            .iter()
            .find(|e| e.generator == generator && e.version == version)
    }
}

/// Our local copy of the remote index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachedIndex {
    /// The `ETag` the server sent with it, to ask later whether it changed.
    pub etag: Option<String>,
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    /// The index as it was downloaded.
    pub index: RemoteIndex,
}

/// One template folder that this crate downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledRecord {
    /// The folder in the templates folder.
    pub dir: String,
    /// The generator's id.
    pub generator: String,
    /// The version.
    pub version: String,
    /// The revision that was downloaded.
    pub revision: u32,
    /// The SHA-256 of the archive that was downloaded.
    pub sha256: String,
}

/// `installed.json`: the template folders this crate downloaded. Everything else in the templates folder belongs to the user
/// and is never touched.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledFile {
    /// The format number of this file.
    pub schema: u32,
    /// One record per downloaded folder.
    pub templates: Vec<InstalledRecord>,
}

impl Default for InstalledFile {
    fn default() -> Self {
        Self {
            schema: INSTALLED_SCHEMA,
            templates: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------- helpers

/// `"1.23.2"` to `(1, 23, 2)`.
pub(crate) fn parse_triplet(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let mut next = || -> Option<u64> {
        let p = parts.next()?;
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let t = (next()?, next()?, next()?);
    parts.next().is_none().then_some(t)
}

/// Numeric comparison, so `1.9.0 < 1.23.0`. Unparsable versions sort first.
pub(crate) fn compare_versions(a: &str, b: &str) -> Ordering {
    parse_triplet(a).cmp(&parse_triplet(b))
}

pub(crate) fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Lower-case hex SHA-256.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn good_index() -> String {
        format!(
            r#"{{ "schema": 1, "templates": [
              {{ "generator": "spfx-webpart", "version": "1.23.2", "revision": 2, "dir": "spfxv.1.23.2",
                 "url": "https://example.com/spfxv.1.23.2.tar.gz", "sha256": "{h}", "size": 1000 }},
              {{ "generator": "spfx-webpart", "version": "1.9.0", "revision": 1, "dir": "spfxv.1.9.0",
                 "url": "https://example.com/spfxv.1.9.0.tar.gz", "sha256": "{h}", "size": 900 }}
            ] }}"#,
            h = "a".repeat(64)
        )
    }

    fn err_of(json: &str) -> String {
        RemoteIndex::from_json(json.as_bytes()).unwrap_err().to_string()
    }

    #[test]
    fn parses_and_sorts_newest_first_numerically() {
        let idx = RemoteIndex::from_json(good_index().as_bytes()).unwrap();
        let versions: Vec<&str> = idx.for_generator("spfx-webpart").iter().map(|e| e.version.as_str()).collect();
        assert_eq!(versions, ["1.23.2", "1.9.0"]); // 1.23 is newer than 1.9
        assert_eq!(idx.find("spfx-webpart", "1.9.0").unwrap().revision, 1);
        assert!(idx.find("spfx-webpart", "9.9.9").is_none());
    }

    #[test]
    fn bad_indexes_are_rejected_with_reasons() {
        let g = good_index();
        assert!(err_of(&g.replace("\"schema\": 1", "\"schema\": 3")).contains("schema 3"));
        assert!(err_of(&g.replace("https://example.com/spfxv.1.9.0", "http://example.com/x")).contains("https://"));
        assert!(err_of(&g.replacen(&"a".repeat(64), "XYZ", 1)).contains("64 lower-case hex"));
        assert!(err_of(&g.replace("\"revision\": 1", "\"revision\": 0")).contains("revision starts at 1"));
        assert!(err_of(&g.replace("spfxv.1.9.0\"", "../evil\"")).contains("plain folder name"));
        assert!(err_of(&g.replace("\"dir\": \"spfxv.1.9.0\"", "\"dir\": \"SPFXV.1.23.2\"")).contains("used twice"));
        assert!(err_of(&g.replace("\"version\": \"1.9.0\"", "\"version\": \"1.23.2\"")).contains("listed twice"));
        assert!(err_of(&g.replace("\"size\": 1000", "\"size\": 0")).contains("size"));
        assert!(err_of(&g.replace("\"size\": 1000", "\"size\": 99999999999")).contains("size"));
        assert!(err_of(&g.replace("spfx-webpart", "SPFx Webpart")).contains("generator"));
        assert!(err_of(&g.replace("\"size\"", "\"sizes\"")).contains("unknown field"));
        assert!(err_of("not json").contains("Invalid template index"));
    }

    #[test]
    fn version_helpers() {
        assert_eq!(parse_triplet("1.23.2"), Some((1, 23, 2)));
        assert_eq!(parse_triplet("1.23"), None);
        assert_eq!(parse_triplet("1.23.2.1"), None);
        assert_eq!(parse_triplet("1.x.2"), None);
        assert_eq!(compare_versions("1.9.0", "1.23.0"), Ordering::Less);
        assert_eq!(compare_versions("2.0.0", "1.99.99"), Ordering::Greater);
    }

    #[test]
    fn sha256_matches_the_known_empty_hash() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(is_sha256_hex(&sha256_hex(b"x")));
        assert!(!is_sha256_hex(&"A".repeat(64)));
    }

    // ------------------------------------------------- schema 2: title, description and tags

    fn with_display(schema: u32, display: &str) -> String {
        good_index()
            .replace("\"schema\": 1", &format!("\"schema\": {schema}"))
            .replacen("\"size\": 1000", &format!("\"size\": 1000{display}"), 1)
    }

    #[test]
    fn both_schemas_are_read_and_the_display_fields_are_optional() {
        // today's index (schema 1, no display fields) still loads
        let old = RemoteIndex::from_json(good_index().as_bytes()).unwrap();
        assert_eq!(old.schema, 1);
        assert!(old.templates.iter().all(|e| !e.has_display() && e.title.is_none() && e.tags.is_empty()));
        // schema 2 without any display field loads too
        assert!(RemoteIndex::from_json(good_index().replace("\"schema\": 1", "\"schema\": 2").as_bytes()).is_ok());
        // schema 2 with them
        let with = with_display(2, r#", "title": "Web part (React)", "description": "A React web part.", "tags": ["spfx", "webpart"]"#);
        let idx = RemoteIndex::from_json(with.as_bytes()).unwrap();
        let e = idx.templates.iter().find(|e| e.has_display()).unwrap();
        assert_eq!(e.title.as_deref(), Some("Web part (React)"));
        assert_eq!(e.description.as_deref(), Some("A React web part."));
        assert_eq!(e.tags, ["spfx", "webpart"]);
        // and writes back without the fields that are not there
        let out = serde_json::to_string(&old).unwrap();
        assert!(!out.contains("title") && !out.contains("tags") && !out.contains("description"), "{out}");
    }

    #[test]
    fn display_fields_need_schema_2_and_sane_values() {
        let d = r#", "title": "T""#;
        assert!(err_of(&with_display(1, d)).contains("need schema 2"));
        assert!(err_of(&with_display(2, r#", "title": "  padded ""#)).contains("title"));
        assert!(err_of(&with_display(2, r#", "title": "two\nlines""#)).contains("control characters"));
        assert!(err_of(&with_display(2, &format!(r#", "title": "{}""#, "x".repeat(81)))).contains("at most 80"));
        assert!(err_of(&with_display(2, &format!(r#", "description": "{}""#, "x".repeat(301)))).contains("at most 300"));
        assert!(err_of(&with_display(2, r#", "tags": ["Has Space"]"#)).contains("tag 'Has Space'"));
        assert!(err_of(&with_display(2, r#", "tags": ["a", "a"]"#)).contains("listed twice"));
        assert!(err_of(&with_display(2, r#", "tags": ["-edge"]"#)).contains("not starting or ending"));
        assert!(err_of(&with_display(2, r#", "tags": ["a","b","c","d","e","f","g","h","i"]"#)).contains("at most 8 tags"));
        // a misspelled field is still refused, as before
        assert!(err_of(&with_display(2, r#", "titel": "T""#)).contains("unknown field"));
    }
}
