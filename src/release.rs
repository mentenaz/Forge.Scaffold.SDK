//! The release builder: generator folders in, `.tar.gz` archives plus one
//! `index.json` out, ready to be uploaded next to each other.
//!
//! This is the other side of [`TemplateStore`](crate::TemplateStore): the store downloads what is
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
use crate::create_vite::{ViteSource, ViteTemplate, CREATE_VITE_DIR, CREATE_VITE_LICENSE};
use crate::upstream::{Overlay, UpstreamSource, UpstreamTemplate, TEMPLATE_FOLDER};
use flate2::{Compression, GzBuilder};
use std::path::{Path, PathBuf};

const MANIFEST_FILE: &str = "manifest.json";
/// The licence file at the root of the SPFx repo, which covers its templates.
const UPSTREAM_LICENSE_FILE: &str = "LICENSE";
/// The notice that travels with every archive of an upstream template.
pub const NOTICE_FILE: &str = "NOTICE.txt";

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
    /// A template of `create-vite`: `repo` is a checkout of the Vite repo and `template` the name of a folder
    /// `template-<template>` in its `packages/create-vite` (for example `react-ts`). It is translated (see
    /// [`crate::create_vite`]) and archived as the generator `vite-<template>`, in a folder called
    /// `vite-<template>.<create-vite version>`. The release is refused without create-vite's LICENSE file.
    CreateVite { repo: PathBuf, template: String },
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
    let repo_source = DirSource::new(repo);
    let (def, mut source) = UpstreamSource::load(&repo_source, &format!("templates/{template}"))?;
    let mut replaced = 0;
    if let Some(dir) = &options.overlay {
        let overlay = Overlay::load(&DirSource::new(dir), "")?;
        replaced = source.apply_overlay(&def, &overlay);
    }

    // The MIT licence requires its notice to be kept in copies of the templates.
    let license = repo_source.read_text(UPSTREAM_LICENSE_FILE).map_err(|_| ScaffoldError::Template {
        path: format!("{}/{UPSTREAM_LICENSE_FILE}", repo.display()),
        reason: "the SPFx repo has no LICENSE file, and its MIT licence requires the notice to travel with the \
                 templates, so nothing was released"
            .into(),
    })?;

    let mut files = vec![
        ArchiveFile { path: MANIFEST_FILE.to_string(), bytes: def.manifest_json().into_bytes() },
        ArchiveFile { path: NOTICE_FILE.to_string(), bytes: upstream_notice(&def, &license, replaced).into_bytes() },
    ];
    for path in source.list_files(TEMPLATE_FOLDER)? {
        let bytes = source.read_file(&format!("{TEMPLATE_FOLDER}/{path}"))?;
        files.push(ArchiveFile { path: format!("{TEMPLATE_FOLDER}/{path}"), bytes });
    }
    Ok((def, files))
}

/// The files of a create-vite template as a generator folder: a generated `manifest.json`, a `NOTICE.txt`
/// with create-vite's licence, and the translated template below [`TEMPLATE_FOLDER`].
fn vite_files(repo: &Path, template: &str, options: &ReleaseOptions) -> Result<(ViteTemplate, Vec<ArchiveFile>)> {
    let repo_source = DirSource::new(repo);
    let (def, mut source) = ViteSource::load(&repo_source, CREATE_VITE_DIR, template)?;
    let mut replaced = 0;
    if let Some(dir) = &options.overlay {
        replaced = source.apply_overlay(&def.template, &Overlay::load(&DirSource::new(dir), "")?);
    }

    // The MIT licence requires its notice to be kept in copies of the templates.
    let license = repo_source.read_text(CREATE_VITE_LICENSE).map_err(|_| ScaffoldError::Template {
        path: format!("{}/{CREATE_VITE_LICENSE}", repo.display()),
        reason: "the Vite repo has no create-vite LICENSE file, and its MIT licence requires the notice to travel with \
                 the templates, so nothing was released"
            .into(),
    })?;

    let mut files = vec![
        ArchiveFile { path: MANIFEST_FILE.to_string(), bytes: def.manifest_json().into_bytes() },
        ArchiveFile { path: NOTICE_FILE.to_string(), bytes: vite_notice(&def, &license, replaced).into_bytes() },
    ];
    for path in source.list_files(TEMPLATE_FOLDER)? {
        let bytes = source.read_file(&format!("{TEMPLATE_FOLDER}/{path}"))?;
        files.push(ArchiveFile { path: format!("{TEMPLATE_FOLDER}/{path}"), bytes });
    }
    Ok((def, files))
}

/// The text of `NOTICE.txt` for a create-vite template. No dates, so the archive stays deterministic.
fn vite_notice(def: &ViteTemplate, license: &str, changed_files: usize) -> String {
    let mut out = format!(
        "This folder is a template made from the \"template-{}\" template (create-vite {}) of\n\
         https://github.com/vitejs/vite (packages/create-vite).\n\
         \n\
         The original files are Copyright (c) 2019-present, VoidZero Inc. and Vite contributors, and are used\n\
         under the MIT licence, whose full text (create-vite's LICENSE file) follows. They were changed in\n\
         three ways: the project name in package.json and in the page title became a token of the Forge\n\
         scaffolding engine, a manifest.json was added, and the files create-vite writes under another name\n\
         (_gitignore, _oxlintrc.json) are renamed when the project is made.\n",
        def.template, def.version
    );
    if changed_files > 0 {
        out.push_str(&format!(
            "\n{changed_files} file(s) (images, the demo page, the README) were replaced or left out by the Forge\n\
             overlay. Those are not part of the Vite project's work and are not covered by the licence below.\n"
        ));
    }
    out.push_str("\n----------------------------------------------------------------------\n\n");
    out.push_str(license.trim_end());
    out.push('\n');
    out
}

/// The text of `NOTICE.txt`: what the files are derived from, what was changed,
/// and Microsoft's licence verbatim. No dates, so the archive stays deterministic.
fn upstream_notice(def: &UpstreamTemplate, license: &str, replaced_images: usize) -> String {
    let mut out = format!(
        "This folder is a template made from the \"{}\" template (template version {}, SPFx {}) of the\n\
         SharePoint Framework templates at https://github.com/SharePoint/spfx.\n\
         \n\
         The original files are Copyright (c) Microsoft Corporation and are used under the MIT licence,\n\
         whose full text follows. They were changed in two ways: the template tags were translated into the\n\
         tokens of the Forge scaffolding engine, and a manifest.json was added.\n",
        def.name, def.version, def.spfx_version
    );
    if replaced_images > 0 {
        out.push_str(&format!(
            "\n{replaced_images} image file(s) were replaced by files of the Forge overlay. Those are not\n\
             part of Microsoft's work and are not covered by the licence below.\n"
        ));
    }
    out.push_str("\n----------------------------------------------------------------------\n\n");
    out.push_str(license.trim_end());
    out.push('\n');
    out
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
        // What a list of templates shows. A generator folder says it in its own manifest; the official SPFx templates and
        // the create-vite templates have none, so it is made from their names and put in the index only (their archives,
        // and so their revisions, are not touched by it).
        let (dir, id, version, files, (title, description, mut tags), (can_create, can_add)) = match input {
            ReleaseInput::Folder(root) => {
                let manifest_text = DirSource::new(root).read_text(MANIFEST_FILE)?;
                let manifest = Manifest::from_json(&manifest_text)?;
                let display = (manifest.title.clone(), manifest.description.clone(), manifest.tags.clone());
                let can = (manifest.can_create(), manifest.can_add());
                (folder_name(root)?, manifest.id, manifest.version, folder_files(root, options)?, display, can)
            }
            ReleaseInput::Upstream { repo, template } => {
                let (def, files) = upstream_files(repo, template, options)?;
                let (title, description, tags) = def.display();
                let dir = format!("{}.{}", def.name, def.spfx_version);
                (dir, def.name, def.spfx_version, files, (Some(title), description, tags), (true, true))
            }
            ReleaseInput::CreateVite { repo, template } => {
                let (def, files) = vite_files(repo, template, options)?;
                let (title, description, tags) = def.display();
                (format!("{}.{}", def.id(), def.version), def.id(), def.version, files, (Some(title), Some(description), tags), (true, false))
            }
        };

        // What the generator can do goes into the index as two tags, so that an app that has not downloaded it can say so,
        // and an older app, which refuses an index with a field it does not know, still reads the index and shows them as tags.
        for (wanted, tag) in [(can_create, crate::manifest::TAG_CAN_CREATE), (can_add, crate::manifest::TAG_CAN_ADD)] {
            if wanted && !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
        }
        if tags.len() > crate::manifest::MAX_TAGS {
            return Err(bad(format!(
                "{dir}: {} tags, and the index carries at most {} (two of them are added by the release: can-create and can-add)",
                tags.len(),
                crate::manifest::MAX_TAGS
            )));
        }

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
            title,
            description,
            tags,
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

    // ----------------------------------------------------------- upstream notice

    const MS_LICENSE: &str = "The projects in this monorepo are licensed under the MIT license.\n\nCopyright (c) Microsoft Corporation. All rights reserved.\n\nMIT License\n\nPermission is hereby granted, free of charge, to any person obtaining a copy.\n";

    /// A tiny stand-in for a checkout of the SPFx repo, with one template.
    fn spfx_repo(with_license: bool) -> Tmp {
        let t = Tmp::new();
        if with_license {
            fs::write(t.0.join("LICENSE"), MS_LICENSE).unwrap();
        }
        let dir = t.0.join("templates/demo");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("template.json"),
            r#"{ "name": "demo-part", "category": "webpart", "version": "0.0.3", "spfxVersion": "1.24.1" }"#,
        )
        .unwrap();
        fs::write(dir.join("src/{componentName.camel}.ts"), "// <%= componentName.pascal %>\n").unwrap();
        fs::write(dir.join("src/index.ts"), "export {};\n").unwrap();
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("assets/logo.png"), [0x89, 0xff]).unwrap();
        t
    }

    fn notice_of(release: &Release) -> String {
        let out = Tmp::new();
        let found = extract_tar_gz(&release.archives[0].bytes, &out.0).unwrap();
        fs::read_to_string(found.join(NOTICE_FILE)).unwrap()
    }

    #[test]
    fn an_upstream_archive_carries_microsofts_notice_and_licence_verbatim() {
        let repo = spfx_repo(true);
        let inputs = [ReleaseInput::Upstream { repo: repo.0.clone(), template: "demo".into() }];
        let release = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap();
        let notice = notice_of(&release);

        assert!(notice.contains("\"demo-part\"") && notice.contains("0.0.3") && notice.contains("SPFx 1.24.1"));
        assert!(notice.contains("https://github.com/SharePoint/spfx"));
        assert!(notice.contains("Copyright (c) Microsoft Corporation"));
        assert!(notice.contains("MIT license") || notice.contains("MIT licence"));
        assert!(notice.contains(MS_LICENSE.trim_end()), "the licence text is included verbatim");
        assert!(!notice.contains("overlay"), "no overlay, so no word about one: {notice}");
        // rebuilding gives the same bytes
        assert_eq!(release.archives[0].bytes, build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap().archives[0].bytes);
    }

    #[test]
    fn the_notice_says_when_images_were_replaced_by_the_overlay() {
        let repo = spfx_repo(true);
        let overlay = Tmp::new();
        fs::write(overlay.0.join("overlay.json"), r#"{ "replace": { "assets/logo.png": "mine.png" } }"#).unwrap();
        fs::write(overlay.0.join("mine.png"), [0x01, 0xff]).unwrap();
        let mut options = ReleaseOptions::new(BASE);
        options.overlay = Some(overlay.0.clone());

        let inputs = [ReleaseInput::Upstream { repo: repo.0.clone(), template: "demo".into() }];
        let notice = notice_of(&build_release_from(&inputs, &options, None).unwrap());
        assert!(notice.contains("1 image file(s) were replaced"), "{notice}");
        assert!(notice.contains("not covered by the licence below"));
    }

    #[test]
    fn an_upstream_template_without_the_licence_file_is_not_released() {
        let repo = spfx_repo(false);
        let inputs = [ReleaseInput::Upstream { repo: repo.0.clone(), template: "demo".into() }];
        let err = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap_err().to_string();
        assert!(err.contains("LICENSE") && err.contains("MIT"), "{err}");
    }

    // ----------------------------------------------------------- create-vite notice

    const VITE_LICENSE: &str = "create-vite is released under the MIT license:\n\nMIT License\n\nCopyright (c) 2019-present, VoidZero Inc. and Vite contributors\n\nPermission is hereby granted, free of charge, to any person obtaining a copy.\n";

    /// A tiny stand-in for a checkout of the Vite repo, with one create-vite template.
    fn vite_repo(with_license: bool) -> Tmp {
        let t = Tmp::new();
        let cv = t.0.join("packages/create-vite");
        let tpl = cv.join("template-react-ts");
        fs::create_dir_all(tpl.join("src")).unwrap();
        if with_license {
            fs::write(cv.join("LICENSE"), VITE_LICENSE).unwrap();
        }
        fs::write(cv.join("package.json"), r#"{ "name": "create-vite", "version": "9.2.1", "engines": { "node": "^20.19.0 || >=22.12.0" } }"#).unwrap();
        fs::write(tpl.join("package.json"), "{\n  \"name\": \"starter\",\n  \"private\": true\n}\n").unwrap();
        fs::write(tpl.join("_gitignore"), "node_modules\n").unwrap();
        fs::write(tpl.join("src/main.tsx"), "export {};\n").unwrap();
        t
    }

    #[test]
    fn a_create_vite_archive_carries_the_licence_and_names_the_generator_after_the_template() {
        let repo = vite_repo(true);
        let inputs = [ReleaseInput::CreateVite { repo: repo.0.clone(), template: "react-ts".into() }];
        let release = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap();

        let entry = &release.index.templates[0];
        assert_eq!((entry.generator.as_str(), entry.version.as_str(), entry.dir.as_str()), ("vite-react-ts", "9.2.1", "vite-react-ts.9.2.1"));
        assert_eq!(release.archives[0].file_name, "vite-react-ts.9.2.1.tar.gz");

        let out = Tmp::new();
        let found = extract_tar_gz(&release.archives[0].bytes, &out.0).unwrap();
        let notice = fs::read_to_string(found.join(NOTICE_FILE)).unwrap();
        assert!(notice.contains("template-react-ts") && notice.contains("create-vite 9.2.1"));
        assert!(notice.contains("https://github.com/vitejs/vite"));
        assert!(notice.contains("VoidZero Inc. and Vite contributors"));
        assert!(notice.contains(VITE_LICENSE.trim_end()), "the licence text is included verbatim");
        // the manifest, the translated files and the notice are all in the archive
        let manifest = Manifest::from_json(&fs::read_to_string(found.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.id, "vite-react-ts");
        assert!(fs::read_to_string(found.join("template/package.json")).unwrap().contains("{__projectName__}"));
        assert!(found.join("template/_gitignore").exists(), "renamed when the project is made, not in the archive");
        // rebuilding gives the same bytes
        assert_eq!(release.archives[0].bytes, build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap().archives[0].bytes);
    }

    #[test]
    fn the_overlay_replaces_vite_images_and_the_notice_says_so() {
        let repo = vite_repo(true);
        fs::write(repo.0.join("packages/create-vite/template-react-ts/src/logo.png"), [0x89, 0xff]).unwrap();
        let overlay = Tmp::new();
        fs::write(overlay.0.join("overlay.json"), r#"{ "replace": { "src/logo.png": "mine.png" } }"#).unwrap();
        fs::write(overlay.0.join("mine.png"), [0x01, 0xff]).unwrap();
        let mut options = ReleaseOptions::new(BASE);
        options.overlay = Some(overlay.0.clone());

        let inputs = [ReleaseInput::CreateVite { repo: repo.0.clone(), template: "react-ts".into() }];
        let release = build_release_from(&inputs, &options, None).unwrap();
        let out = Tmp::new();
        let found = extract_tar_gz(&release.archives[0].bytes, &out.0).unwrap();
        let notice = fs::read_to_string(found.join(NOTICE_FILE)).unwrap();
        assert!(notice.contains("1 file(s) (images, the demo page, the README) were replaced or left out"), "{notice}");
        assert!(notice.contains("not covered by the licence below"));
        assert_eq!(fs::read(found.join("template/src/logo.png")).unwrap(), [0x01, 0xff]);
        // without an overlay there is no word about one
        let plain = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap();
        let out2 = Tmp::new();
        let found2 = extract_tar_gz(&plain.archives[0].bytes, &out2.0).unwrap();
        assert!(!fs::read_to_string(found2.join(NOTICE_FILE)).unwrap().contains("overlay"));
    }

    #[test]
    fn a_create_vite_template_without_the_licence_file_is_not_released() {
        let repo = vite_repo(false);
        let inputs = [ReleaseInput::CreateVite { repo: repo.0.clone(), template: "react-ts".into() }];
        let err = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap_err().to_string();
        assert!(err.contains("LICENSE") && err.contains("MIT"), "{err}");
    }

    // ------------------------------------------------- schema 2: what the entries show

    #[test]
    fn a_generator_folder_gives_its_own_display_text_and_the_others_get_it_from_their_names() {
        // a generator folder says it in its manifest
        let (_t, root) = generator("1.0.0");
        let text = fs::read_to_string(root.join("manifest.json")).unwrap().replace(
            "\"id\": \"demo\",",
            "\"id\": \"demo\", \"title\": \"Demo\", \"description\": \"A demo generator.\", \"tags\": [\"demo\", \"test\"],",
        );
        fs::write(root.join("manifest.json"), text).unwrap();
        let spfx = spfx_repo(true);
        let vite = vite_repo(true);
        let inputs = [
            ReleaseInput::Folder(root.clone()),
            ReleaseInput::Upstream { repo: spfx.0.clone(), template: "demo".into() },
            ReleaseInput::CreateVite { repo: vite.0.clone(), template: "react-ts".into() },
        ];
        let release = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap();
        assert_eq!(release.index.schema, 2);
        let by = |g: &str| release.index.templates.iter().find(|e| e.generator == g).unwrap();

        let own = by("demo");
        assert_eq!((own.title.as_deref(), own.description.as_deref()), (Some("Demo"), Some("A demo generator.")));
        assert_eq!(own.tags, ["demo", "test", "can-create"], "what it can do is added as tags");
        let official = by("demo-part");
        assert_eq!(official.title.as_deref(), Some("Demo part"));
        assert_eq!(official.tags, ["spfx", "webpart", "can-create", "can-add"]);
        let vite_entry = by("vite-react-ts");
        assert_eq!(vite_entry.title.as_deref(), Some("Vite + React (TypeScript)"));
        assert_eq!(vite_entry.tags, ["vite", "react", "typescript", "frontend", "can-create"]);
        // the whole index passes its own validation, and survives being written and read
        let again = RemoteIndex::from_json(serde_json::to_string(&release.index).unwrap().as_bytes()).unwrap();
        assert_eq!(again, release.index);
    }

    #[test]
    fn an_add_on_says_it_can_be_added_and_not_that_it_can_make_a_project() {
        let (_t, root) = generator("1.0.0");
        let text = fs::read_to_string(root.join("manifest.json")).unwrap();
        let text = text.replace("\"folderFrom\": \"name\",", "").replace(
            "\"id\": \"demo\",",
            "\"id\": \"demo\", \"add\": { \"mode\": \"render\" },",
        );
        assert!(!text.contains("folderFrom"), "the fixture has no folderFrom: {text}");
        fs::write(root.join("manifest.json"), text).unwrap();
        let release = build_release_from(&[ReleaseInput::Folder(root)], &ReleaseOptions::new(BASE), None).unwrap();
        assert_eq!(release.index.templates[0].tags, ["can-add"]);
    }

    #[test]
    fn a_generator_with_too_many_tags_of_its_own_is_refused_with_the_reason() {
        let (_t, root) = generator("1.0.0");
        let tags = (1..=8).map(|i| format!("\"t{i}\"")).collect::<Vec<_>>().join(", ");
        let text = fs::read_to_string(root.join("manifest.json")).unwrap().replace(
            "\"id\": \"demo\",",
            &format!("\"id\": \"demo\", \"tags\": [{tags}],"),
        );
        fs::write(root.join("manifest.json"), text).unwrap();
        let err = build_release_from(&[ReleaseInput::Folder(root)], &ReleaseOptions::new(BASE), None).unwrap_err().to_string();
        assert!(err.contains("can-create") && err.contains("at most 8"), "{err}");
    }

    #[test]
    fn display_text_for_the_official_and_vite_templates_does_not_change_their_archives() {
        // the same archive bytes as without the display text: only the index carries it, so no revision moves
        let spfx = spfx_repo(true);
        let vite = vite_repo(true);
        let inputs = [
            ReleaseInput::Upstream { repo: spfx.0.clone(), template: "demo".into() },
            ReleaseInput::CreateVite { repo: vite.0.clone(), template: "react-ts".into() },
        ];
        let first = build_release_from(&inputs, &ReleaseOptions::new(BASE), None).unwrap();
        let second = build_release_from(&inputs, &ReleaseOptions::new(BASE), Some(&first.index)).unwrap();
        assert!(second.index.templates.iter().all(|e| e.revision == 1), "nothing was bumped: {:?}", second.index.templates);
        for (a, b) in first.archives.iter().zip(&second.archives) {
            assert_eq!(a.bytes, b.bytes);
        }
        let manifest_of = |r: &Release, i: usize| {
            let out = Tmp::new();
            let found = extract_tar_gz(&r.archives[i].bytes, &out.0).unwrap();
            fs::read_to_string(found.join("manifest.json")).unwrap()
        };
        assert!(!manifest_of(&first, 0).contains("\"title\"") && !manifest_of(&first, 1).contains("\"title\""));
    }
}
