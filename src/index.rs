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

/// The only index format this crate understands.
pub const INDEX_SCHEMA: u32 = 1;
/// Upper bound for an index download.
pub const MAX_INDEX_BYTES: usize = 1024 * 1024;
/// Upper bound for one template archive.
pub const MAX_ARCHIVE_BYTES: u64 = 25 * 1024 * 1024;

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
    pub schema: u32,
    pub templates: Vec<IndexEntry>,
}

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
}

impl RemoteIndex {
    /// Parses and validates an index. All problems are reported together.
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

    pub fn validate(&self) -> Result<()> {
        let mut issues = Vec::new();
        if self.schema != INDEX_SCHEMA {
            issues.push(format!(
                "schema {} is not supported (expected {INDEX_SCHEMA})",
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
    pub etag: Option<String>,
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    pub index: RemoteIndex,
}

/// One template folder that this crate downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledRecord {
    pub dir: String,
    pub generator: String,
    pub version: String,
    pub revision: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledFile {
    pub schema: u32,
    pub templates: Vec<InstalledRecord>,
}

impl Default for InstalledFile {
    fn default() -> Self {
        Self {
            schema: INDEX_SCHEMA,
            templates: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------- helpers

/// `"1.23.2"` to `(1, 23, 2)`.
pub fn parse_triplet(s: &str) -> Option<(u64, u64, u64)> {
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
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    parse_triplet(a).cmp(&parse_triplet(b))
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
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
        assert!(err_of(&g.replace("\"schema\": 1", "\"schema\": 2")).contains("schema 2"));
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
}
