//! Template packs: one generator version in a single file.
//!
//! A pack holds the manifest and every template file, so the templates can be
//! compiled into the app (`include_str!`) or downloaded as one small file, and
//! `plan()` can read them without any folder on disk. The format is described
//! by `pack.v1.json` (JSON Schema); this module is the strict reader.
//!
//! File contents are either inline in the entry, or stored once in `blobs`
//! under their SHA-256 and referenced by that hash, so a file that appears in
//! many variants (a logo, say) costs its size once.
//!
//! A pack is plain data. Loading it checks everything up front: the schema
//! version, safe and unique paths, base64, every hash, size limits, and that
//! the manifest only refers to folders and files that exist in the pack.

use crate::error::{Result, ScaffoldError};
use crate::index::{is_sha256_hex, sha256_hex};
use crate::manifest::Manifest;
use crate::source::TemplateSource;
use crate::stage::normalize_rel;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Bound;
use std::sync::Arc;

/// The pack format this crate reads.
pub const PACK_SCHEMA: u32 = 1;

/// Upper bounds applied while loading. The defaults are far above any real
/// template and exist to stop a damaged or hostile pack from using all memory.
#[derive(Debug, Clone, Copy)]
pub struct PackLimits {
    /// Number of entries (files and folders).
    pub max_entries: usize,
    /// Size of one file after decoding.
    pub max_file_bytes: usize,
    /// Sum of all file sizes after decoding, counting a shared blob once per
    /// file that uses it (that is what `plan()` would read).
    pub max_total_bytes: usize,
}

impl Default for PackLimits {
    fn default() -> Self {
        Self {
            max_entries: 20_000,
            max_file_bytes: 20 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPack {
    #[serde(rename = "$schema", default)]
    _schema_url: Option<String>,
    schema: u32,
    generator: String,
    version: String,
    revision: u32,
    manifest: Manifest,
    #[serde(default)]
    blobs: BTreeMap<String, RawBlob>,
    entries: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlob {
    encoding: Encoding,
    content: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawEntry {
    Dir(RawDir),
    File(RawFile),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDir {
    path: String,
}

/// Inline form: `encoding` + `content` (+ optional `sha256`).
/// Shared form: `blob` only.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    #[serde(default)]
    encoding: Option<Encoding>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    blob: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Encoding {
    Utf8,
    Base64,
}

/// A validated pack, ready to be used as a [`TemplateSource`].
pub struct Pack {
    generator: String,
    version: String,
    revision: u32,
    manifest: Manifest,
    files: BTreeMap<String, Arc<Vec<u8>>>,
    dirs: BTreeSet<String>,
    stored_bytes: u64,
}

impl fmt::Debug for Pack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pack")
            .field("generator", &self.generator)
            .field("version", &self.version)
            .field("revision", &self.revision)
            .field("files", &self.files.len())
            .finish()
    }
}

fn bad(msg: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Pack(msg.into())
}

/// `a/b/c` → `a`, `a/b` (every parent folder).
fn parents(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..i])
}

fn decode(encoding: Encoding, content: String, what: &str) -> Result<Vec<u8>> {
    match encoding {
        Encoding::Utf8 => Ok(content.into_bytes()),
        Encoding::Base64 => STANDARD
            .decode(content.as_bytes())
            .map_err(|e| bad(format!("{what}: invalid base64: {e}"))),
    }
}

impl Pack {
    /// Parses and validates a pack with the default limits.
    pub fn from_json(json: &str) -> Result<Pack> {
        Self::from_json_with_limits(json, PackLimits::default())
    }

    /// Parses and validates a pack.
    pub fn from_json_with_limits(json: &str, limits: PackLimits) -> Result<Pack> {
        let raw: RawPack = serde_json::from_str(json).map_err(|e| bad(e.to_string()))?;

        if raw.schema != PACK_SCHEMA {
            return Err(bad(format!(
                "unsupported pack schema {} (this build reads {PACK_SCHEMA})",
                raw.schema
            )));
        }
        raw.manifest.validate()?;
        if raw.generator != raw.manifest.id {
            return Err(bad(format!(
                "generator '{}' does not match the manifest id '{}'",
                raw.generator, raw.manifest.id
            )));
        }
        if raw.version != raw.manifest.version {
            return Err(bad(format!(
                "version '{}' does not match the manifest version '{}'",
                raw.version, raw.manifest.version
            )));
        }
        if raw.revision == 0 {
            return Err(bad("revision must be 1 or higher"));
        }
        if raw.entries.len() > limits.max_entries {
            return Err(bad(format!("too many entries (more than {})", limits.max_entries)));
        }

        // Shared blobs: decode once, check that the key is the hash of the content.
        let mut blobs: BTreeMap<String, Arc<Vec<u8>>> = BTreeMap::new();
        let mut stored: u64 = 0;
        for (key, blob) in raw.blobs {
            if !is_sha256_hex(&key) {
                return Err(bad(format!("blob key '{key}' must be 64 lower-case hex characters")));
            }
            let bytes = decode(blob.encoding, blob.content, &format!("blob {key}"))?;
            if bytes.len() > limits.max_file_bytes {
                return Err(bad(format!(
                    "blob {key} is larger than {} bytes",
                    limits.max_file_bytes
                )));
            }
            let actual = sha256_hex(&bytes);
            if actual != key {
                return Err(bad(format!(
                    "blob {key}: the content hashes to {actual}"
                )));
            }
            stored += bytes.len() as u64;
            blobs.insert(key, Arc::new(bytes));
        }
        let mut used_blobs: BTreeSet<String> = BTreeSet::new();

        let mut files: BTreeMap<String, Arc<Vec<u8>>> = BTreeMap::new();
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        // Lower-case view, because Windows compares names without case.
        let mut lower_files: BTreeMap<String, String> = BTreeMap::new();
        let mut total: usize = 0;

        for entry in raw.entries {
            match entry {
                RawEntry::Dir(d) => {
                    let path = normalize_rel(&d.path).map_err(|e| bad(e.to_string()))?;
                    for p in parents(&path) {
                        dirs.insert(p.to_string());
                    }
                    dirs.insert(path);
                }
                RawEntry::File(f) => {
                    let path = normalize_rel(&f.path).map_err(|e| bad(e.to_string()))?;
                    let bytes: Arc<Vec<u8>> = match (f.blob, f.encoding, f.content) {
                        (Some(key), None, None) => {
                            if f.sha256.is_some() {
                                return Err(bad(format!(
                                    "'{path}': a blob reference must not also carry a sha256"
                                )));
                            }
                            let bytes = blobs.get(&key).ok_or_else(|| {
                                bad(format!("'{path}': blob {key} is not in the pack"))
                            })?;
                            used_blobs.insert(key);
                            Arc::clone(bytes)
                        }
                        (None, Some(encoding), Some(content)) => {
                            let bytes = decode(encoding, content, &format!("'{path}'"))?;
                            if bytes.len() > limits.max_file_bytes {
                                return Err(bad(format!(
                                    "'{path}' is larger than {} bytes",
                                    limits.max_file_bytes
                                )));
                            }
                            if let Some(expected) = &f.sha256 {
                                if !is_sha256_hex(expected) {
                                    return Err(bad(format!(
                                        "'{path}': sha256 must be 64 lower-case hex characters"
                                    )));
                                }
                                let actual = sha256_hex(&bytes);
                                if &actual != expected {
                                    return Err(bad(format!(
                                        "'{path}': sha256 mismatch (expected {expected}, got {actual})"
                                    )));
                                }
                            }
                            stored += bytes.len() as u64;
                            Arc::new(bytes)
                        }
                        _ => {
                            return Err(bad(format!(
                                "'{path}': use either 'blob', or 'encoding' with 'content'"
                            )))
                        }
                    };
                    total += bytes.len();
                    if total > limits.max_total_bytes {
                        return Err(bad(format!(
                            "the files add up to more than {} bytes",
                            limits.max_total_bytes
                        )));
                    }
                    if let Some(first) = lower_files.insert(path.to_lowercase(), path.clone()) {
                        return Err(bad(format!(
                            "'{path}' appears twice (also as '{first}'; names are compared without case)"
                        )));
                    }
                    for p in parents(&path) {
                        dirs.insert(p.to_string());
                    }
                    files.insert(path, bytes);
                }
            }
        }

        if let Some(unused) = blobs.keys().find(|k| !used_blobs.contains(*k)) {
            return Err(bad(format!("blob {unused} is not used by any file")));
        }

        // A name cannot be a file and a folder at the same time.
        let lower_dirs: BTreeSet<String> = dirs.iter().map(|d| d.to_lowercase()).collect();
        for (lower, original) in &lower_files {
            if lower_dirs.contains(lower) {
                return Err(bad(format!("'{original}' is both a file and a folder")));
            }
        }

        let pack = Pack {
            generator: raw.generator,
            version: raw.version,
            revision: raw.revision,
            manifest: raw.manifest,
            files,
            dirs,
            stored_bytes: stored,
        };
        pack.check_references()?;
        Ok(pack)
    }

    /// Every folder and file the manifest names must exist in the pack, even
    /// for branches whose `when` is false: a pack that only works for some
    /// answers is broken.
    fn check_references(&self) -> Result<()> {
        let m = &self.manifest;
        let folder = |what: &str, name: &str| -> Result<()> {
            let n = normalize_rel(name).map_err(|e| bad(format!("{what} '{name}': {e}")))?;
            if self.dirs.contains(&n) {
                Ok(())
            } else {
                Err(bad(format!("{what} '{name}' is not in the pack")))
            }
        };
        let file = |what: &str, name: &str| -> Result<()> {
            let n = normalize_rel(name).map_err(|e| bad(format!("{what} '{name}': {e}")))?;
            if self.files.contains_key(&n) {
                Ok(())
            } else {
                Err(bad(format!("{what} '{name}' is not in the pack")))
            }
        };
        for i in &m.include {
            folder("include folder", &i.folder)?;
        }
        for r in &m.repeat {
            folder("repeat folder", &r.folder)?;
        }
        for p in &m.parts {
            file("part source", &p.source)?;
        }
        for mg in &m.merge {
            if let Some(vf) = &mg.value_file {
                file("merge valueFile", vf)?;
            }
        }
        Ok(())
    }

    pub fn generator(&self) -> &str {
        &self.generator
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn revision(&self) -> u32 {
        self.revision
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
    /// Sum of all file sizes, as `plan()` would read them.
    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|b| b.len() as u64).sum()
    }
    /// Bytes the pack actually stores: a shared blob counts once. This is the
    /// number to watch for the size budget.
    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }
}

impl TemplateSource for Pack {
    fn list_files(&self, folder: &str) -> Result<Vec<String>> {
        let folder = normalize_rel(folder)?;
        if !self.dirs.contains(&folder) {
            return Err(ScaffoldError::Template {
                path: folder,
                reason: "template folder not found in the pack".into(),
            });
        }
        let prefix = format!("{folder}/");
        let mut out: Vec<String> = self
            .files
            .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, _)| k[prefix.len()..].to_string())
            .collect();
        // Same order as a folder listing: compare name by name.
        out.sort_by_key(|p| p.replace('/', "\u{0}"));
        Ok(out)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let path = normalize_rel(path)?;
        self.files
            .get(&path)
            .map(|b| b.as_ref().clone())
            .ok_or_else(|| ScaffoldError::Template {
                path,
                reason: "file not found in the pack".into(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn manifest() -> Value {
        json!({
            "id": "demo", "version": "1.0.0", "folderFrom": "name",
            "prompts": [ { "key": "name", "type": "text" } ],
            "include": [ { "folder": "base" } ]
        })
    }

    fn file(path: &str, content: &str) -> Value {
        json!({ "type": "file", "path": path, "encoding": "utf8", "content": content })
    }

    fn pack_with(entries: Vec<Value>) -> String {
        json!({
            "schema": 1, "generator": "demo", "version": "1.0.0", "revision": 1,
            "manifest": manifest(), "entries": entries
        })
        .to_string()
    }

    fn err_of(json: &str) -> String {
        Pack::from_json(json).unwrap_err().to_string()
    }

    #[test]
    fn loads_text_and_binary_files() {
        let entries = vec![
            file("base/a.txt", "hello"),
            json!({ "type": "file", "path": "base/img/logo.bin", "encoding": "base64",
                    "content": STANDARD.encode([0xff, 0xfe, 0x00]) }),
        ];
        let p = Pack::from_json(&pack_with(entries)).unwrap();
        assert_eq!((p.generator(), p.version(), p.revision()), ("demo", "1.0.0", 1));
        assert_eq!(p.file_count(), 2);
        assert_eq!(p.read_file("base/a.txt").unwrap(), b"hello");
        assert_eq!(p.read_file("base/img/logo.bin").unwrap(), vec![0xff, 0xfe, 0x00]);
        assert!(p.read_text("base/img/logo.bin").is_err());
        assert!(p.read_file("base/missing").is_err());
    }

    #[test]
    fn listing_is_relative_and_ordered_like_a_folder() {
        let entries = vec![
            file("base/a.txt", "1"),
            file("base/a/x.ts", "2"),
            file("base/b.txt", "3"),
            file("other/skip.txt", "4"),
        ];
        let p = Pack::from_json(&pack_with(entries)).unwrap();
        // A folder listing puts the folder `a` before the file `a.txt`.
        assert_eq!(p.list_files("base").unwrap(), vec!["a/x.ts", "a.txt", "b.txt"]);
        assert_eq!(p.list_files("base/a").unwrap(), vec!["x.ts"]);
    }

    #[test]
    fn empty_folders_exist_and_missing_ones_do_not() {
        let entries = vec![file("base/a.txt", "1"), json!({ "type": "dir", "path": "base/empty" })];
        let p = Pack::from_json(&pack_with(entries)).unwrap();
        assert!(p.list_files("base/empty").unwrap().is_empty());
        assert!(p.list_files("base/nope").is_err());
    }

    #[test]
    fn checks_sha256_when_present() {
        let good = json!({ "type": "file", "path": "base/a.txt", "encoding": "utf8",
                           "sha256": sha256_hex(b"hello"), "content": "hello" });
        assert!(Pack::from_json(&pack_with(vec![good])).is_ok());

        let wrong = json!({ "type": "file", "path": "base/a.txt", "encoding": "utf8",
                            "sha256": sha256_hex(b"other"), "content": "hello" });
        assert!(err_of(&pack_with(vec![wrong])).contains("sha256 mismatch"));

        let malformed = json!({ "type": "file", "path": "base/a.txt", "encoding": "utf8",
                               "sha256": "9f2c…", "content": "hello" });
        assert!(err_of(&pack_with(vec![malformed])).contains("64 lower-case hex"));
    }

    #[test]
    fn rejects_unsafe_and_conflicting_paths() {
        assert!(err_of(&pack_with(vec![file("../evil.txt", "x")])).contains("'..'"));
        assert!(err_of(&pack_with(vec![file("/abs.txt", "x")])).contains("relative"));
        assert!(err_of(&pack_with(vec![file("base/a.txt", "1"), file("base/A.TXT", "2")]))
            .contains("appears twice"));
        assert!(err_of(&pack_with(vec![file("base/a", "1"), file("base/a/b.txt", "2")]))
            .contains("both a file and a folder"));
    }

    #[test]
    fn rejects_bad_base64_and_unknown_fields() {
        let bad64 = json!({ "type": "file", "path": "base/x", "encoding": "base64", "content": "***" });
        assert!(err_of(&pack_with(vec![bad64])).contains("invalid base64"));

        let extra = json!({ "type": "file", "path": "base/x", "encoding": "utf8", "content": "", "mode": 755 });
        assert!(Pack::from_json(&pack_with(vec![extra])).is_err(), "unknown entry field");

        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["surprise"] = json!(true);
        assert!(Pack::from_json(&v.to_string()).is_err(), "unknown pack field");
    }

    #[test]
    fn header_must_match_the_manifest() {
        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["schema"] = json!(2);
        assert!(err_of(&v.to_string()).contains("unsupported pack schema"));

        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["generator"] = json!("other");
        assert!(err_of(&v.to_string()).contains("does not match the manifest id"));

        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["version"] = json!("2.0.0");
        assert!(err_of(&v.to_string()).contains("does not match the manifest version"));

        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["revision"] = json!(0);
        assert!(err_of(&v.to_string()).contains("revision"));
    }

    #[test]
    fn invalid_manifest_is_rejected() {
        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["manifest"]["version"] = json!("1.0");
        v["version"] = json!("1.0");
        assert!(err_of(&v.to_string()).contains("version"));
    }

    #[test]
    fn manifest_references_must_exist_in_the_pack() {
        // 'include' names folder 'base', but the pack only has 'other'.
        assert!(err_of(&pack_with(vec![file("other/a.txt", "1")])).contains("include folder 'base'"));

        // A part source and a valueFile must exist too, even behind a false `when`.
        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["manifest"]["prompts"] = json!([
            { "key": "name", "type": "text" },
            { "key": "flag", "type": "bool", "default": false }
        ]);
        v["manifest"]["parts"] = json!([{ "target": "out.txt", "source": "parts/missing.txt",
                                           "when": { "key": "flag", "equals": true } }]);
        assert!(err_of(&v.to_string()).contains("part source"));
    }

    #[test]
    fn schema_url_is_accepted() {
        let mut v: Value = serde_json::from_str(&pack_with(vec![file("base/a", "1")])).unwrap();
        v["$schema"] = json!("https://schemas.mentenaz-server.com/forge-scaffold/pack.v1.json");
        v["manifest"]["$schema"] =
            json!("https://schemas.mentenaz-server.com/forge-scaffold/manifest.v1.json");
        assert!(Pack::from_json(&v.to_string()).is_ok());
    }

    #[test]
    fn debug_does_not_dump_contents() {
        let p = Pack::from_json(&pack_with(vec![file("base/a", "secret-content")])).unwrap();
        assert!(!format!("{p:?}").contains("secret-content"));
    }
    // ------------------------------------------------------- shared blobs

    fn pack_with_blobs(blobs: Value, entries: Vec<Value>) -> String {
        json!({
            "schema": 1, "generator": "demo", "version": "1.0.0", "revision": 1,
            "manifest": manifest(), "blobs": blobs, "entries": entries
        })
        .to_string()
    }

    fn blob(content: &str) -> (String, Value) {
        (
            sha256_hex(content.as_bytes()),
            json!({ "encoding": "utf8", "content": content }),
        )
    }

    fn by_blob(path: &str, key: &str) -> Value {
        json!({ "type": "file", "path": path, "blob": key })
    }

    #[test]
    fn identical_files_share_one_stored_blob() {
        let (key, b) = blob("shared logo bytes");
        let json = pack_with_blobs(
            json!({ key.clone(): b }),
            vec![
                by_blob("base/a/logo.svg", &key),
                by_blob("base/b/logo.svg", &key),
                file("base/readme.txt", "hi"),
            ],
        );
        let p = Pack::from_json(&json).unwrap();
        assert_eq!(p.file_count(), 3);
        assert_eq!(p.read_file("base/a/logo.svg").unwrap(), b"shared logo bytes");
        assert_eq!(p.read_file("base/b/logo.svg").unwrap(), b"shared logo bytes");
        let blob_len = "shared logo bytes".len() as u64;
        assert_eq!(p.total_bytes(), 2 * blob_len + 2, "what plan() reads");
        assert_eq!(p.stored_bytes(), blob_len + 2, "the shared file counts once");
    }

    #[test]
    fn binary_blobs_roundtrip() {
        let bytes = [0xff_u8, 0xfe, 0x00, 0x7b];
        let key = sha256_hex(&bytes);
        let json = pack_with_blobs(
            json!({ key.clone(): { "encoding": "base64", "content": STANDARD.encode(bytes) } }),
            vec![by_blob("base/x.bin", &key)],
        );
        assert_eq!(Pack::from_json(&json).unwrap().read_file("base/x.bin").unwrap(), bytes);
    }

    #[test]
    fn blob_problems_are_rejected() {
        let (key, b) = blob("content");

        // key is not the hash of the content
        let wrong_key = sha256_hex(b"something else");
        let j = pack_with_blobs(json!({ wrong_key.clone(): b }), vec![by_blob("base/a", &wrong_key)]);
        assert!(err_of(&j).contains("hashes to"));

        // malformed key
        let j = pack_with_blobs(json!({ "abc": b }), vec![by_blob("base/a", "abc")]);
        assert!(err_of(&j).contains("64 lower-case hex"));

        // reference to a blob that is not there
        let j = pack_with_blobs(json!({}), vec![by_blob("base/a", &key)]);
        assert!(err_of(&j).contains("is not in the pack"));

        // blob nobody uses
        let j = pack_with_blobs(json!({ key.clone(): b }), vec![file("base/a", "1")]);
        assert!(err_of(&j).contains("not used by any file"));

        // a reference must not mix with inline fields or a sha256
        let mixed = json!({ "type": "file", "path": "base/a", "blob": key, "encoding": "utf8", "content": "x" });
        let j = pack_with_blobs(json!({ key.clone(): b }), vec![mixed]);
        assert!(err_of(&j).contains("either 'blob'"));
        let with_hash = json!({ "type": "file", "path": "base/a", "blob": key, "sha256": key });
        let j = pack_with_blobs(json!({ key: b }), vec![with_hash]);
        assert!(err_of(&j).contains("must not also carry a sha256"));

        // an inline file needs both encoding and content
        let half = json!({ "type": "file", "path": "base/a", "encoding": "utf8" });
        assert!(err_of(&pack_with(vec![half])).contains("either 'blob'"));
        let empty = json!({ "type": "file", "path": "base/a" });
        assert!(err_of(&pack_with(vec![empty])).contains("either 'blob'"));
    }

    #[test]
    fn limits_count_every_use_of_a_shared_blob() {
        let (key, b) = blob("0123456789"); // 10 bytes
        let json = pack_with_blobs(
            json!({ key.clone(): b }),
            vec![by_blob("base/a", &key), by_blob("base/b", &key), by_blob("base/c", &key)],
        );
        let limits = PackLimits { max_total_bytes: 25, ..PackLimits::default() };
        assert!(Pack::from_json_with_limits(&json, limits).unwrap_err().to_string().contains("add up to more than"));
        let roomy = PackLimits { max_total_bytes: 30, ..PackLimits::default() };
        assert!(Pack::from_json_with_limits(&json, roomy).is_ok());

        let tiny_file = PackLimits { max_file_bytes: 5, ..PackLimits::default() };
        assert!(Pack::from_json_with_limits(&json, tiny_file).unwrap_err().to_string().contains("larger than"));

        let few = PackLimits { max_entries: 2, ..PackLimits::default() };
        assert!(Pack::from_json_with_limits(&json, few).unwrap_err().to_string().contains("too many entries"));
    }
}
