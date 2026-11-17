//! The packer: a generator folder in, one `pack.v1.json` out.
//!
//! The folder holds `manifest.json` plus the template folders it refers to
//! (the same layout `plan()` reads through [`DirSource`]). The pack embeds the
//! manifest, and every other file becomes an entry: UTF-8 text as `utf8`,
//! anything else as `base64`. Content that appears in more than one file is
//! stored once in `blobs` under its SHA-256.
//!
//! The output is deterministic (entries sorted by path, blobs by hash), so the
//! same folder always gives the same bytes. Before returning, the result is
//! loaded again with [`Pack::from_json`], so the packer cannot produce a pack
//! the loader rejects.
//!
//! Empty folders are not kept (a folder exists in a pack only through the files
//! below it). The packer expects a generator folder, not a raw `yo` capture:
//! it walks everything below the root before applying the exclude list.

use crate::error::{Result, ScaffoldError};
use crate::index::sha256_hex;
use crate::manifest::Manifest;
use crate::pack::{Pack, PACK_SCHEMA};
use crate::source::{DirSource, TemplateSource};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

const SCHEMA_URL: &str = "https://schemas.mentenaz-server.com/forge-scaffold/pack.v1.json";
const MANIFEST_FILE: &str = "manifest.json";

/// Folder and file names left out of the pack, matched against every part of a path.
pub const DEFAULT_EXCLUDES: &[&str] = &[".git", "node_modules", ".DS_Store", "Thumbs.db"];

#[derive(Debug, Clone)]
pub struct PackOptions {
    /// Revision of this build of the generator version. 1 or higher.
    pub revision: u32,
    /// Names (not patterns) skipped wherever they appear in a path.
    pub exclude: Vec<String>,
    /// Turn `\r\n` into `\n` in text files, so a Windows checkout and a Linux
    /// checkout give the same pack.
    pub normalize_newlines: bool,
}

impl PackOptions {
    pub fn new(revision: u32) -> Self {
        Self {
            revision,
            exclude: DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
            normalize_newlines: true,
        }
    }
}

/// One file ready to be written into the pack.
struct Item {
    path: String,
    bytes: Vec<u8>,
    is_text: bool,
}

/// Builds the pack JSON for the generator folder at `root`.
pub fn pack_dir(root: &Path, options: &PackOptions) -> Result<String> {
    if options.revision == 0 {
        return Err(ScaffoldError::Pack("revision must be 1 or higher".into()));
    }
    let source = DirSource::new(root);

    let manifest_text = source.read_text(MANIFEST_FILE)?;
    let manifest = Manifest::from_json(&manifest_text)?;
    let mut manifest_value: Value = serde_json::from_str(&manifest_text)
        .map_err(|e| ScaffoldError::Manifest(e.to_string()))?;
    if let Some(obj) = manifest_value.as_object_mut() {
        obj.remove("$schema"); // editor hint only, not part of the pack
    }

    let mut items: Vec<Item> = Vec::new();
    for path in source.list_files("")? {
        if path == MANIFEST_FILE || path.split('/').any(|part| options.exclude.iter().any(|e| e == part)) {
            continue;
        }
        let mut bytes = source.read_file(&path)?;
        let is_text = std::str::from_utf8(&bytes).is_ok();
        if is_text && options.normalize_newlines && bytes.contains(&b'\r') {
            bytes = normalize_newlines(&bytes);
        }
        items.push(Item { path, bytes, is_text });
    }

    // Content used by more than one file goes into `blobs`.
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    let hashes: Vec<String> = items.iter().map(|i| sha256_hex(&i.bytes)).collect();
    for h in &hashes {
        *uses.entry(h.clone()).or_default() += 1;
    }

    let mut blobs = Map::new();
    let mut entries = Vec::with_capacity(items.len());
    for (item, hash) in items.iter().zip(&hashes) {
        let (encoding, content) = encode(item);
        if uses[hash] > 1 {
            blobs
                .entry(hash.clone())
                .or_insert_with(|| json!({ "encoding": encoding, "content": content }));
            entries.push(json!({ "type": "file", "path": item.path, "blob": hash }));
        } else {
            entries.push(json!({
                "type": "file",
                "path": item.path,
                "encoding": encoding,
                "sha256": hash,
                "content": content,
            }));
        }
    }

    let mut pack = Map::new();
    pack.insert("$schema".into(), json!(SCHEMA_URL));
    pack.insert("schema".into(), json!(PACK_SCHEMA));
    pack.insert("generator".into(), json!(manifest.id));
    pack.insert("version".into(), json!(manifest.version));
    pack.insert("revision".into(), json!(options.revision));
    pack.insert("manifest".into(), manifest_value);
    if !blobs.is_empty() {
        pack.insert("blobs".into(), Value::Object(blobs));
    }
    pack.insert("entries".into(), Value::Array(entries));

    let mut out = serde_json::to_string_pretty(&Value::Object(pack))
        .map_err(|e| ScaffoldError::Pack(e.to_string()))?;
    out.push('\n');

    // The loader is the judge: never hand out a pack it would refuse.
    Pack::from_json(&out)?;
    Ok(out)
}

fn encode(item: &Item) -> (&'static str, String) {
    if item.is_text {
        ("utf8", String::from_utf8_lossy(&item.bytes).into_owned())
    } else {
        ("base64", STANDARD.encode(&item.bytes))
    }
}

pub(crate) fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1; // drop the \r, keep the \n
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "forge-scaffold-packer-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, bytes: &[u8]) {
            let full = self.0.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, bytes).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const MANIFEST: &str = r#"{
      "$schema": "https://example.test/manifest.v1.json",
      "id": "demo",
      "version": "1.2.3",
      "prompts": [ { "key": "name", "type": "text" } ],
      "folderFrom": "name",
      "include": [ { "folder": "base" } ]
    }"#;

    fn generator() -> Tmp {
        let t = Tmp::new();
        t.write("manifest.json", MANIFEST.as_bytes());
        t.write("base/readme.md", b"# {__name__}\n");
        t.write("base/src/index.ts", b"export {};\n");
        t.write("base/assets/icon.png", &[0x89, b'P', b'N', b'G', 0xff, 0x00]);
        t
    }

    #[test]
    fn packs_and_loads_back() {
        let t = generator();
        let json = pack_dir(&t.0, &PackOptions::new(3)).unwrap();
        let pack = Pack::from_json(&json).unwrap();
        assert_eq!(pack.generator(), "demo");
        assert_eq!(pack.version(), "1.2.3");
        assert_eq!(pack.revision(), 3);
        assert_eq!(pack.file_count(), 3);
        assert_eq!(pack.read_file("base/assets/icon.png").unwrap(), vec![0x89, b'P', b'N', b'G', 0xff, 0x00]);
        assert_eq!(pack.read_text("base/readme.md").unwrap(), "# {__name__}\n");
    }

    #[test]
    fn output_is_deterministic() {
        let t = generator();
        let a = pack_dir(&t.0, &PackOptions::new(1)).unwrap();
        let b = pack_dir(&t.0, &PackOptions::new(1)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn repeated_content_is_stored_once() {
        let t = generator();
        t.write("base/a/logo.bin", &[1, 2, 3, 0xff]);
        t.write("base/b/logo.bin", &[1, 2, 3, 0xff]);
        let json = pack_dir(&t.0, &PackOptions::new(1)).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["blobs"].as_object().unwrap().len(), 1);
        let pack = Pack::from_json(&json).unwrap();
        assert_eq!(pack.read_file("base/a/logo.bin").unwrap(), vec![1, 2, 3, 0xff]);
        assert_eq!(pack.read_file("base/b/logo.bin").unwrap(), vec![1, 2, 3, 0xff]);
    }

    #[test]
    fn excluded_names_are_left_out_anywhere_in_a_path() {
        let t = generator();
        t.write("base/node_modules/x/index.js", b"x");
        t.write("base/.git/config", b"x");
        t.write("base/Thumbs.db", b"x");
        let pack = Pack::from_json(&pack_dir(&t.0, &PackOptions::new(1)).unwrap()).unwrap();
        assert_eq!(pack.file_count(), 3);
    }

    #[test]
    fn newlines_are_normalized_only_when_asked() {
        let t = generator();
        t.write("base/win.txt", b"a\r\nb\r\n");
        let on = Pack::from_json(&pack_dir(&t.0, &PackOptions::new(1)).unwrap()).unwrap();
        assert_eq!(on.read_text("base/win.txt").unwrap(), "a\nb\n");
        let mut keep = PackOptions::new(1);
        keep.normalize_newlines = false;
        let off = Pack::from_json(&pack_dir(&t.0, &keep).unwrap()).unwrap();
        assert_eq!(off.read_text("base/win.txt").unwrap(), "a\r\nb\r\n");
    }

    #[test]
    fn binary_files_keep_their_bytes_even_with_normalization_on() {
        let t = generator();
        t.write("base/blob.bin", &[0x0d, 0x0a, 0xff]); // CR LF then an invalid UTF-8 byte
        let pack = Pack::from_json(&pack_dir(&t.0, &PackOptions::new(1)).unwrap()).unwrap();
        assert_eq!(pack.read_file("base/blob.bin").unwrap(), vec![0x0d, 0x0a, 0xff]);
    }

    #[test]
    fn a_folder_the_manifest_needs_must_exist() {
        let t = Tmp::new();
        t.write("manifest.json", MANIFEST.as_bytes()); // no base/ folder
        assert!(pack_dir(&t.0, &PackOptions::new(1)).is_err());
    }

    #[test]
    fn missing_or_invalid_manifest_and_zero_revision_are_errors() {
        let t = Tmp::new();
        assert!(pack_dir(&t.0, &PackOptions::new(1)).is_err());
        let t = generator();
        assert!(pack_dir(&t.0, &PackOptions::new(0)).is_err());
    }
}
