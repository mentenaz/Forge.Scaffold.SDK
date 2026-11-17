//! The release builder: generator folders in, `.tar.gz` archives plus one
//! `index.json` out, ready to be uploaded next to each other.
//!
//! This is the other side of [`crate::store`]: the store downloads what is
//! built here. Nothing is uploaded by this crate; the host or a CI job copies
//! the returned bytes where they are served from.
//!
//! * Archives are deterministic: entries sorted by path, no times, owners or
//!   permission bits that depend on the machine, a fixed gzip header. The same
//!   folder gives the same bytes, so the `sha256` in the index only changes
//!   when the content does.
//! * Every archive has one top-level folder named after the generator folder
//!   (`spfxv.1.23.2/manifest.json`, ...), which is what the extractor and the
//!   index's `dir` field expect.
//! * The limits the extractor applies are checked while building, so the
//!   builder never produces an archive the store would refuse.
//! * `revision` is kept when an archive is byte-identical to the one in the
//!   previous index, and bumped by one when it is not.

use crate::archive::{MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES};
use crate::error::{Result, ScaffoldError};
use crate::index::{compare_versions, sha256_hex, IndexEntry, RemoteIndex, INDEX_SCHEMA, MAX_ARCHIVE_BYTES};
use crate::manifest::Manifest;
use crate::packer::{normalize_newlines, DEFAULT_EXCLUDES};
use crate::source::{DirSource, TemplateSource};
use crate::upstream::{Overlay, UpstreamSource, UpstreamTemplate, TEMPLATE_FOLDER};
use flate2::{Compression, GzBuilder};
use std::path::{Path, PathBuf};

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone)]
pub struct ReleaseOptions {
    /// Where the archives will be served from, e.g.
    /// `https://downloads.mentenaz-server.com/forge-scaffold`. Must be `https`.
    pub base_url: String,
    /// Names (not patterns) skipped wherever they appear in a path.
    pub exclude: Vec<String>,
    /// Turn `\r\n` into `\n` in text files, so a Windows checkout and a Linux
    /// checkout give the same archive.
    pub normalize_newlines: bool,
    /// A folder with an `overlay.json` (see [`Overlay`]) whose files replace
    /// files of upstream templates, such as your own logo and icons. Only used
    /// for [`ReleaseInput::Upstream`].
    pub overlay: Option<PathBuf>,
}

impl ReleaseOptions {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            exclude: DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
            normalize_newlines: true,
            overlay: None,
        }
    }
}

/// Where a template of the release comes from.
#[derive(Debug, Clone)]
pub enum ReleaseInput {
    /// A generator folder (`manifest.json` plus template folders).
    Folder(PathBuf),
    /// A template of the official SharePoint/spfx CLI: `repo` is a checkout of
    /// it and `template` a folder name below its `templates/`. It is translated
    /// (see [`crate::upstream`]) and archived as a generator named after the
    /// template, in a folder called `<template>.<spfx version>`.
    Upstream { repo: PathBuf, template: String },
}

/// One built archive and the file name it must be uploaded as.
#[derive(Debug, Clone)]
pub struct ReleaseArchive {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Everything to upload: the archives, then (last) the index.
#[derive(Debug, Clone)]
pub struct Release {
    pub index: RemoteIndex,
    pub archives: Vec<ReleaseArchive>,
}

impl Release {
    /// The text of `index.json`.
    pub fn index_json(&self) -> Result<String> {
        let mut out = serde_json::to_string_pretty(&self.index)
            .map_err(|e| ScaffoldError::Index(e.to_string()))?;
        out.push('\n');
        Ok(out)
    }
}

fn bad(msg: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Archive(msg.into())
}

/// The name of the generator folder, which becomes the archive's top folder
/// and the index's `dir`.
fn folder_name(root: &Path) -> Result<String> {
    root.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| bad(format!("'{}' has no usable folder name", root.display())))
}

/// One file going into an archive: its path below the archive's top folder.
struct ArchiveFile {
    path: String,
    bytes: Vec<u8>,
}

/// Writes `files` into a deterministic `.tar.gz` under the top folder `top`.
fn pack_files(top: &str, mut files: Vec<ArchiveFile>, options: &ReleaseOptions) -> Result<Vec<u8>> {
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.len() > MAX_FILES {
        return Err(bad(format!("more than {MAX_FILES} files in {top}")));
    }

    let gz = GzBuilder::new()
        .mtime(0)
        .operating_system(255) // "unknown": the same header on every platform
        .write(Vec::new(), Compression::best());
    let mut tar = tar::Builder::new(gz);
    let mut total: u64 = 0;

    for file in files {
        let mut bytes = file.bytes;
        if options.normalize_newlines && bytes.contains(&b'\r') && std::str::from_utf8(&bytes).is_ok() {
            bytes = normalize_newlines(&bytes);
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(bad(format!("'{}' is larger than {MAX_FILE_BYTES} bytes", file.path)));
        }
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(bad(format!("{top} is larger than {MAX_TOTAL_BYTES} bytes")));
        }

        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        tar.append_data(&mut header, format!("{top}/{}", file.path), bytes.as_slice())
            .map_err(|e| bad(format!("cannot add '{}': {e}", file.path)))?;
    }

    let gz = tar.into_inner().map_err(|e| bad(e.to_string()))?;
    gz.finish().map_err(|e| bad(e.to_string()))
}

fn folder_files(root: &Path, options: &ReleaseOptions) -> Result<Vec<ArchiveFile>> {
    let source = DirSource::new(root);
    let mut files = Vec::new();
    for path in source.list_files("")? {
        if path.split('/').any(|part| options.exclude.iter().any(|e| e == part)) {
            continue;
        }
        let bytes = source.read_file(&path)?;
        files.push(ArchiveFile { path, bytes });
    }
    Ok(files)
}

/// The files of an upstream template as a generator folder: a generated
/// `manifest.json` and the translated template below [`TEMPLATE_FOLDER`], with
/// the overlay from `options` applied.
fn upstream_files(
    repo: &Path,
    template: &str,
    options: &ReleaseOptions,
) -> Result<(UpstreamTemplate, Vec<ArchiveFile>)> {
    let (def, mut source) = UpstreamSource::load(&DirSource::new(repo), &format!("templates/{template}"))?;
    if let Some(dir) = &options.overlay {
        let overlay = Overlay::load(&DirSource::new(dir), "")?;
        source.apply_overlay(&def, &overlay);
    }

    let mut files = vec![ArchiveFile {
        path: MANIFEST_FILE.to_string(),
        bytes: def.manifest_json().into_bytes(),
    }];
    for path in source.list_files(TEMPLATE_FOLDER)? {
        let bytes = source.read_file(&format!("{TEMPLATE_FOLDER}/{path}"))?;
        files.push(ArchiveFile { path: format!("{TEMPLATE_FOLDER}/{path}"), bytes });
    }
    Ok((def, files))
}

/// Builds the `.tar.gz` for one generator folder.
pub fn build_archive(root: &Path, options: &ReleaseOptions) -> Result<Vec<u8>> {
    let top = folder_name(root)?;
    pack_files(&top, folder_files(root, options)?, options)
}

/// Builds the archives and the index for a set of generator folders.
///
/// `previous` is the index published last time, if any: it decides each
/// entry's `revision`. Every folder becomes `<folder name>.tar.gz`.
pub fn build_release(
    generators: &[PathBuf],
    options: &ReleaseOptions,
    previous: Option<&RemoteIndex>,
) -> Result<Release> {
    let inputs: Vec<ReleaseInput> = generators.iter().cloned().map(ReleaseInput::Folder).collect();
    build_release_from(&inputs, options, previous)
}

/// Like [`build_release`], for any mix of generator folders and upstream templates.
pub fn build_release_from(
    inputs: &[ReleaseInput],
    options: &ReleaseOptions,
    previous: Option<&RemoteIndex>,
) -> Result<Release> {
    let base = options.base_url.trim_end_matches('/');
    if !base.starts_with("https://") {
        return Err(ScaffoldError::Index("the base url must start with https://".into()));
    }

    let mut entries = Vec::with_capacity(inputs.len());
    let mut archives = Vec::with_capacity(inputs.len());

    for input in inputs {
        let (dir, id, version, files) = match input {
            ReleaseInput::Folder(root) => {
                let manifest_text = DirSource::new(root).read_text(MANIFEST_FILE)?;
                let manifest = Manifest::from_json(&manifest_text)?;
                (folder_name(root)?, manifest.id, manifest.version, folder_files(root, options)?)
            }
            ReleaseInput::Upstream { repo, template } => {
                let (def, files) = upstream_files(repo, template, options)?;
                (format!("{}.{}", def.name, def.spfx_version), def.name, def.spfx_version, files)
            }
        };

        let bytes = pack_files(&dir, files, options)?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(bad(format!("{dir}.tar.gz is larger than {MAX_ARCHIVE_BYTES} bytes")));
        }
        let sha256 = sha256_hex(&bytes);
        let file_name = format!("{dir}.tar.gz");

        let revision = match previous.and_then(|p| p.find(&id, &version)) {
            Some(old) if old.sha256 == sha256 => old.revision,
            Some(old) => old.revision + 1,
            None => 1,
        };

        entries.push(IndexEntry {
            generator: id,
            version,
            revision,
            dir,
            url: format!("{base}/{file_name}"),
            sha256,
            size: bytes.len() as u64,
        });
        archives.push(ReleaseArchive { file_name, bytes });
    }

    entries.sort_by(|a, b| {
        a.generator
            .cmp(&b.generator)
            .then_with(|| compare_versions(&a.version, &b.version))
            .then_with(|| a.dir.cmp(&b.dir))
    });
    archives.sort_by(|a, b| a.file_name.cmp(&b.file_name));

    let index = RemoteIndex {
        schema: INDEX_SCHEMA,
        templates: entries,
    };
    index.validate()?;
    Ok(Release { index, archives })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::extract_tar_gz;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "forge-scaffold-release-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const BASE: &str = "https://downloads.example.test/forge-scaffold";

    fn manifest(version: &str) -> String {
        format!(
            r#"{{ "id": "demo", "version": "{version}", "folderFrom": "name",
                 "prompts": [ {{ "key": "name", "type": "text" }} ],
                 "include": [ {{ "folder": "base" }} ] }}"#
        )
    }

    /// A generator folder called `demo.<version>` inside a scratch folder.
    fn generator(version: &str) -> (Tmp, PathBuf) {
        let t = Tmp::new();
        let root = t.0.join(format!("demo.{version}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("manifest.json"), manifest(version)).unwrap();
        fs::create_dir_all(root.join("base/src")).unwrap();
        fs::write(root.join("base/readme.md"), "# {__name__}\n").unwrap();
        fs::write(root.join("base/src/index.ts"), "export {};\n").unwrap();
        fs::write(root.join("base/logo.png"), [0x89, b'P', b'N', b'G', 0xff, 0x00]).unwrap();
        (t, root)
    }

    #[test]
    fn archive_extracts_back_to_the_folder() {
        let (_t, root) = generator("1.0.0");
        let bytes = build_archive(&root, &ReleaseOptions::new(BASE)).unwrap();

        let out = Tmp::new();
        let found = extract_tar_gz(&bytes, &out.0).unwrap();
        assert_eq!(found.file_name().unwrap(), "demo.1.0.0");
        assert_eq!(fs::read_to_string(found.join("base/readme.md")).unwrap(), "# {__name__}\n");
        assert_eq!(fs::read(found.join("base/logo.png")).unwrap(), vec![0x89, b'P', b'N', b'G', 0xff, 0x00]);
        assert!(found.join("manifest.json").is_file());
    }

    #[test]
    fn archive_bytes_are_deterministic() {
        let (_t, root) = generator("1.0.0");
        let o = ReleaseOptions::new(BASE);
        assert_eq!(build_archive(&root, &o).unwrap(), build_archive(&root, &o).unwrap());
    }

    #[test]
    fn long_paths_survive() {
        let (_t, root) = generator("1.0.0");
        let long = format!("base/{}/file.txt", "deep-folder-name/".repeat(8).trim_end_matches('/'));
        assert!(long.len() > 100);
        fs::create_dir_all(root.join(&long).parent().unwrap()).unwrap();
        fs::write(root.join(&long), "x").unwrap();
        let bytes = build_archive(&root, &ReleaseOptions::new(BASE)).unwrap();
        let out = Tmp::new();
        let found = extract_tar_gz(&bytes, &out.0).unwrap();
        assert_eq!(fs::read_to_string(found.join(&long)).unwrap(), "x");
    }

    #[test]
    fn excluded_names_and_newlines() {
        let (_t, root) = generator("1.0.0");
        fs::create_dir_all(root.join("base/node_modules/x")).unwrap();
        fs::write(root.join("base/node_modules/x/i.js"), "x").unwrap();
        fs::write(root.join("base/win.txt"), "a\r\nb\r\n").unwrap();
        let out = Tmp::new();
        let found = extract_tar_gz(&build_archive(&root, &ReleaseOptions::new(BASE)).unwrap(), &out.0).unwrap();
        assert!(!found.join("base/node_modules").exists());
        assert_eq!(fs::read_to_string(found.join("base/win.txt")).unwrap(), "a\nb\n");

        let mut keep = ReleaseOptions::new(BASE);
        keep.normalize_newlines = false;
        let out2 = Tmp::new();
        let found2 = extract_tar_gz(&build_archive(&root, &keep).unwrap(), &out2.0).unwrap();
        assert_eq!(fs::read_to_string(found2.join("base/win.txt")).unwrap(), "a\r\nb\r\n");
    }

    #[test]
    fn index_lists_every_archive_with_hash_size_and_url() {
        let (_t1, a) = generator("1.9.0");
        let (_t2, b) = generator("1.23.0");
        let release = build_release(&[b.clone(), a.clone()], &ReleaseOptions::new(format!("{BASE}/")), None).unwrap();

        // numeric order, not text order
        let versions: Vec<_> = release.index.templates.iter().map(|e| e.version.as_str()).collect();
        assert_eq!(versions, vec!["1.9.0", "1.23.0"]);
        for e in &release.index.templates {
            let archive = release.archives.iter().find(|a| a.file_name == format!("{}.tar.gz", e.dir)).unwrap();
            assert_eq!(e.sha256, sha256_hex(&archive.bytes));
            assert_eq!(e.size, archive.bytes.len() as u64);
            assert_eq!(e.url, format!("{BASE}/{}.tar.gz", e.dir));
            assert_eq!(e.revision, 1);
        }
        // what we publish is what the client parses
        RemoteIndex::from_json(release.index_json().unwrap().as_bytes()).unwrap();
    }

    #[test]
    fn revision_is_kept_when_unchanged_and_bumped_when_changed() {
        let (_t, root) = generator("1.0.0");
        let o = ReleaseOptions::new(BASE);
        let first = build_release(std::slice::from_ref(&root), &o, None).unwrap();

        let same = build_release(std::slice::from_ref(&root), &o, Some(&first.index)).unwrap();
        assert_eq!(same.index.templates[0].revision, 1);
        assert_eq!(same.index, first.index);

        fs::write(root.join("base/readme.md"), "# changed\n").unwrap();
        let changed = build_release(&[root], &o, Some(&first.index)).unwrap();
        assert_eq!(changed.index.templates[0].revision, 2);
        assert_ne!(changed.index.templates[0].sha256, first.index.templates[0].sha256);
    }

    #[test]
    fn bad_input_is_refused() {
        let (_t, root) = generator("1.0.0");
        assert!(build_release(std::slice::from_ref(&root), &ReleaseOptions::new("http://insecure.test"), None).is_err());

        // two folders for the same generator version
        let (_t2, dup) = generator("1.0.0");
        let o = ReleaseOptions::new(BASE);
        assert!(build_release(&[root, dup], &o, None).is_err());

        // a folder without a manifest
        let empty = Tmp::new();
        assert!(build_release(std::slice::from_ref(&empty.0), &o, None).is_err());
    }
}
