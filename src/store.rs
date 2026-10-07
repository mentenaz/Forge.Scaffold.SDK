//! The templates folder: what is installed, what can be downloaded, and how it
//! is kept up to date.
//!
//! ```text
//! <templates root>/
//!   index.json          our cached copy of the remote index (+ ETag, last check)
//!   installed.json      the folders this crate downloaded
//!   spfxv.1.23.2/       a template version (manifest.json + template folders)
//!   my-own-spfx/        a folder you made yourself: never touched, wins over downloads
//!   .update.lock        present while an update runs
//! ```
//!
//! A folder is *managed* only if `installed.json` lists it. Everything else is
//! the user's own work and is never overwritten, replaced or deleted.

use crate::archive::extract_tar_gz;
use crate::error::{Result, ScaffoldError};
use crate::fetch::{Fetch, FetchResult};
use crate::index::{
    compare_versions, sha256_hex, CachedIndex, IndexEntry, InstalledFile, InstalledRecord,
    RemoteIndex, MAX_ARCHIVE_BYTES, MAX_INDEX_BYTES,
};
use crate::manifest::Manifest;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INDEX_FILE: &str = "index.json";
const INSTALLED_FILE: &str = "installed.json";
const LOCK_FILE: &str = ".update.lock";
/// A lock older than this is assumed to be left over from a crash.
const STALE_LOCK: Duration = Duration::from_secs(10 * 60);

/// Environment variable that overrides the default templates folder.
pub const ENV_TEMPLATES_ROOT: &str = "FORGE_TEMPLATES";

/// Seconds since the Unix epoch. Pass this as `now` to the update functions.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where the templates live: the path the host passes in, else
/// `FORGE_TEMPLATES`, else `<data dir>/mentenaz/templates`.
pub fn resolve_templates_root(explicit: Option<&Path>) -> Result<PathBuf> {
    resolve_root_with(explicit, |k| std::env::var(k).ok(), dirs::data_dir())
}

fn resolve_root_with(
    explicit: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
    data_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(v) = env(ENV_TEMPLATES_ROOT).filter(|v| !v.trim().is_empty()) {
        return Ok(PathBuf::from(v));
    }
    data_dir
        .map(|d| d.join("mentenaz").join("templates"))
        .ok_or_else(|| ScaffoldError::NotFound("cannot determine a data folder for the templates".into()))
}

// ------------------------------------------------------------------ types

/// One row in the version dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: String,
    /// Folder name on disk, when present.
    pub dir: Option<String>,
    /// The folder is there (downloaded or local).
    pub installed: bool,
    /// The folder is the user's own (not downloaded by us). It always wins.
    pub local: bool,
    pub installed_revision: Option<u32>,
    /// Revision in the cached index.
    pub available_revision: Option<u32>,
    /// A downloaded folder is behind the index.
    pub update_available: bool,
    /// Can be downloaded (listed in the index and not shadowed by a local folder).
    pub downloadable: bool,
}

/// `list_versions` result: newest first, plus folders that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionList {
    pub versions: Vec<VersionInfo>,
    pub problems: Vec<String>,
}

/// A template version ready to be given to `plan()`.
#[derive(Debug, Clone)]
pub struct ResolvedTemplate {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// `None` for local folders.
    pub revision: Option<u32>,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// The last check is recent enough; nothing was downloaded.
    Skipped,
    /// The server said nothing changed.
    NotModified,
    /// A new index was downloaded.
    Refreshed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateKind {
    /// The newest version of a generator is not installed yet.
    NewVersion,
    /// An installed version has a newer revision.
    Revised { from: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub generator: String,
    pub version: String,
    pub revision: u32,
    pub kind: UpdateKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub status: CheckStatus,
    pub updates: Vec<AvailableUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub generator: String,
    pub version: String,
    pub revision: u32,
    pub dir: PathBuf,
    /// The revision that was replaced, if this was an update.
    pub replaced_revision: Option<u32>,
}

/// The panel's toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSettings {
    /// Download new templates automatically after a check.
    pub auto_update: bool,
    /// Minimum time between checks. Default: 24 hours.
    pub check_interval_secs: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_update: true,
            check_interval_secs: 24 * 60 * 60,
        }
    }
}

/// Progress for the panel while `auto_update` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEvent {
    Checking,
    Checked(CheckOutcome),
    Downloading { generator: String, version: String },
    Installed(InstallOutcome),
    /// One download failed; the others still run.
    Failed {
        generator: String,
        version: String,
        error: String,
    },
}

// ------------------------------------------------------------------ store

pub struct TemplateStore {
    root: PathBuf,
}

/// A folder in the templates root that holds a readable `manifest.json`.
struct Found {
    dir_name: String,
    path: PathBuf,
    manifest: Manifest,
    record: Option<InstalledRecord>,
}

impl TemplateStore {
    /// Opens (and creates) the templates folder.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(io_err(&root))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Versions of one generator for the dropdown: newest first, from the
    /// folders on disk and the cached index. Works offline.
    pub fn list_versions(&self, generator: &str) -> Result<VersionList> {
        let (found, problems) = self.scan()?;
        let cache = self.read_cache();
        let remote: Vec<&IndexEntry> = cache
            .as_ref()
            .map(|c| c.index.for_generator(generator))
            .unwrap_or_default();

        let mut versions: BTreeSet<String> = BTreeSet::new();
        for f in found.iter().filter(|f| f.manifest.id == generator) {
            versions.insert(f.manifest.spfx_version.clone());
        }
        for e in &remote {
            versions.insert(e.version.clone());
        }
        let mut versions: Vec<String> = versions.into_iter().collect();
        versions.sort_by(|a, b| compare_versions(b, a));

        let list = versions
            .into_iter()
            .map(|version| {
                let here = pick(&found, generator, &version);
                let entry = remote.iter().find(|e| e.version == version);
                let local = here.is_some_and(|f| f.record.is_none());
                let installed_revision = here.and_then(|f| f.record.as_ref().map(|r| r.revision));
                let available_revision = entry.map(|e| e.revision);
                VersionInfo {
                    dir: here.map(|f| f.dir_name.clone()),
                    installed: here.is_some(),
                    local,
                    installed_revision,
                    available_revision,
                    update_available: matches!(
                        (installed_revision, available_revision),
                        (Some(have), Some(avail)) if avail > have
                    ),
                    downloadable: entry.is_some() && !local,
                    version,
                }
            })
            .collect();

        Ok(VersionList {
            versions: list,
            problems,
        })
    }

    /// Finds the template folder for a version. A local folder wins over a
    /// downloaded one. Fails while an update is running, so a plan never reads
    /// a folder that is being swapped.
    pub fn resolve(&self, generator: &str, version: &str) -> Result<ResolvedTemplate> {
        if lock_is_fresh(&self.root.join(LOCK_FILE)) {
            return Err(ScaffoldError::Busy(
                "A template update is running, try again in a moment".into(),
            ));
        }
        let (mut found, _) = self.scan()?;
        let idx = best_index(&found, generator, version).ok_or_else(|| {
            ScaffoldError::NotFound(format!("template {generator} {version} is not installed"))
        })?;
        let f = found.swap_remove(idx);
        Ok(ResolvedTemplate {
            revision: f.record.as_ref().map(|r| r.revision),
            local: f.record.is_none(),
            dir: f.path,
            manifest: f.manifest,
        })
    }

    /// Checks the index for changes. Respects `interval_secs` unless `force`
    /// (the "check now" button). On a network error the cached data is left as it is.
    pub fn check_updates(
        &self,
        fetch: &dyn Fetch,
        index_url: &str,
        now: u64,
        force: bool,
        interval_secs: u64,
    ) -> Result<CheckOutcome> {
        if !index_url.starts_with("https://") {
            return Err(ScaffoldError::Index("the index url must start with https://".into()));
        }
        let cache = self.read_cache();

        if let Some(c) = &cache {
            let due = force || now < c.checked_at || now - c.checked_at >= interval_secs;
            if !due {
                return Ok(CheckOutcome {
                    status: CheckStatus::Skipped,
                    updates: self.pending_updates()?,
                });
            }
        }

        let etag = cache.as_ref().and_then(|c| c.etag.as_deref());
        let status = match fetch.get(index_url, etag)? {
            FetchResult::NotModified => {
                let mut c = cache.ok_or_else(|| {
                    ScaffoldError::Index("the server answered 'not modified' but there is no cached index".into())
                })?;
                c.checked_at = now;
                self.write_json(INDEX_FILE, &c)?;
                CheckStatus::NotModified
            }
            FetchResult::Body { bytes, etag } => {
                if bytes.len() > MAX_INDEX_BYTES {
                    return Err(ScaffoldError::Index(format!(
                        "the index is larger than {MAX_INDEX_BYTES} bytes"
                    )));
                }
                let index = RemoteIndex::from_json(&bytes)?;
                self.write_json(
                    INDEX_FILE,
                    &CachedIndex {
                        etag,
                        checked_at: now,
                        index,
                    },
                )?;
                CheckStatus::Refreshed
            }
        };
        Ok(CheckOutcome {
            status,
            updates: self.pending_updates()?,
        })
    }

    /// What `auto_update` would download: revisions of installed versions, and
    /// the newest version of each generator when it is not installed.
    pub fn pending_updates(&self) -> Result<Vec<AvailableUpdate>> {
        let Some(cache) = self.read_cache() else {
            return Ok(Vec::new());
        };
        let (found, _) = self.scan()?;
        let mut out = Vec::new();

        for e in &cache.index.templates {
            let here = pick(&found, &e.generator, &e.version);
            if let Some(rec) = here.and_then(|f| f.record.as_ref()) {
                if e.revision > rec.revision {
                    out.push(AvailableUpdate {
                        generator: e.generator.clone(),
                        version: e.version.clone(),
                        revision: e.revision,
                        kind: UpdateKind::Revised { from: rec.revision },
                    });
                }
            }
        }

        let generators: BTreeSet<&str> = cache.index.templates.iter().map(|e| e.generator.as_str()).collect();
        for g in generators {
            if let Some(newest) = cache.index.for_generator(g).first() {
                if pick(&found, g, &newest.version).is_none() {
                    out.push(AvailableUpdate {
                        generator: g.to_string(),
                        version: newest.version.clone(),
                        revision: newest.revision,
                        kind: UpdateKind::NewVersion,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Downloads (or updates) one version listed in the cached index.
    pub fn update(&self, fetch: &dyn Fetch, generator: &str, version: &str) -> Result<InstallOutcome> {
        let cache = self.read_cache().ok_or_else(|| {
            ScaffoldError::NotFound("there is no template index yet, check for updates first".into())
        })?;
        let entry = cache.index.find(generator, version).cloned().ok_or_else(|| {
            ScaffoldError::NotFound(format!("template {generator} {version} is not in the index"))
        })?;
        self.install(fetch, &entry)
    }

    /// Checks (respecting the interval) and, if `settings.auto_update`, installs
    /// everything pending. Meant to run on a background thread at start-up.
    pub fn auto_update(
        &self,
        fetch: &dyn Fetch,
        index_url: &str,
        settings: UpdateSettings,
        now: u64,
        mut on_event: impl FnMut(UpdateEvent),
    ) -> Result<()> {
        on_event(UpdateEvent::Checking);
        let outcome = self.check_updates(fetch, index_url, now, false, settings.check_interval_secs)?;
        let pending = outcome.updates.clone();
        on_event(UpdateEvent::Checked(outcome));
        if !settings.auto_update {
            return Ok(());
        }
        for u in pending {
            on_event(UpdateEvent::Downloading {
                generator: u.generator.clone(),
                version: u.version.clone(),
            });
            match self.update(fetch, &u.generator, &u.version) {
                Ok(done) => on_event(UpdateEvent::Installed(done)),
                Err(e) => on_event(UpdateEvent::Failed {
                    generator: u.generator,
                    version: u.version,
                    error: e.to_string(),
                }),
            }
        }
        Ok(())
    }

    // -------------------------------------------------------- installing

    fn install(&self, fetch: &dyn Fetch, entry: &IndexEntry) -> Result<InstallOutcome> {
        let _lock = LockGuard::acquire(&self.root.join(LOCK_FILE))?;

        // Never overwrite a folder the user made.
        let mut installed = self.read_installed();
        let target = self.root.join(&entry.dir);
        let previous = installed.templates.iter().find(|r| r.dir == entry.dir).cloned();
        if target.exists() && previous.is_none() {
            return Err(ScaffoldError::LocalTemplate(format!(
                "The folder '{}' is not managed by Forge.Scaffold and was left untouched",
                entry.dir
            )));
        }

        let bytes = match fetch.get(&entry.url, None)? {
            FetchResult::Body { bytes, .. } => bytes,
            FetchResult::NotModified => {
                return Err(ScaffoldError::Fetch {
                    url: entry.url.clone(),
                    reason: "unexpected 'not modified' for an unconditional request".into(),
                })
            }
        };
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(ScaffoldError::Archive(format!(
                "the download is larger than {MAX_ARCHIVE_BYTES} bytes"
            )));
        }
        let actual = sha256_hex(&bytes);
        if actual != entry.sha256 {
            return Err(ScaffoldError::ChecksumMismatch {
                url: entry.url.clone(),
                expected: entry.sha256.clone(),
                actual,
            });
        }

        // Extract next to the final place, so the final move is a rename on one volume.
        let temp = TempFolder::create(&self.root)?;
        let template_root = extract_tar_gz(&bytes, temp.path())?;
        let manifest_text = fs::read_to_string(template_root.join("manifest.json"))
            .map_err(io_err(&template_root))?;
        let manifest = Manifest::from_json(&manifest_text)?;
        if manifest.id != entry.generator || manifest.spfx_version != entry.version {
            return Err(ScaffoldError::Archive(format!(
                "the archive holds {} {} but the index promised {} {}",
                manifest.id, manifest.spfx_version, entry.generator, entry.version
            )));
        }

        swap_into_place(&self.root, &template_root, &target)?;

        installed.templates.retain(|r| r.dir != entry.dir);
        installed.templates.push(InstalledRecord {
            dir: entry.dir.clone(),
            generator: entry.generator.clone(),
            version: entry.version.clone(),
            revision: entry.revision,
            sha256: entry.sha256.clone(),
        });
        self.write_json(INSTALLED_FILE, &installed)?;

        Ok(InstallOutcome {
            generator: entry.generator.clone(),
            version: entry.version.clone(),
            revision: entry.revision,
            dir: target,
            replaced_revision: previous.map(|r| r.revision),
        })
    }

    // ----------------------------------------------------------- helpers

    fn scan(&self) -> Result<(Vec<Found>, Vec<String>)> {
        let installed = self.read_installed();
        let mut found = Vec::new();
        let mut problems = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&self.root)
            .map_err(io_err(&self.root))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let manifest_path = e.path().join("manifest.json");
            if !manifest_path.is_file() {
                continue;
            }
            let parsed = fs::read_to_string(&manifest_path)
                .map_err(|err| err.to_string())
                .and_then(|t| Manifest::from_json(&t).map_err(|err| err.to_string()));
            match parsed {
                Ok(manifest) => found.push(Found {
                    record: installed.templates.iter().find(|r| r.dir == name).cloned(),
                    dir_name: name,
                    path: e.path(),
                    manifest,
                }),
                Err(err) => problems.push(format!("{name}: {err}")),
            }
        }
        Ok((found, problems))
    }

    fn read_cache(&self) -> Option<CachedIndex> {
        self.read_json(INDEX_FILE).ok().flatten()
    }

    fn read_installed(&self) -> InstalledFile {
        self.read_json(INSTALLED_FILE).ok().flatten().unwrap_or_default()
    }

    fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.root.join(name);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| ScaffoldError::Index(format!("{name}: {e}"))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ScaffoldError::Io { path, source: e }),
        }
    }

    /// Writes next to the file and renames, so a crash never leaves half a file.
    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.root.join(name);
        let tmp = self.root.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
        let text = serde_json::to_string_pretty(value).map_err(|e| ScaffoldError::Index(e.to_string()))?;
        fs::write(&tmp, text).map_err(io_err(&tmp))?;
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            ScaffoldError::Io { path, source: e }
        })
    }
}

/// The folder to use for a version: a local one if there is one.
fn pick<'a>(found: &'a [Found], generator: &str, version: &str) -> Option<&'a Found> {
    best_index(found, generator, version).map(|i| &found[i])
}

fn best_index(found: &[Found], generator: &str, version: &str) -> Option<usize> {
    let matches = |f: &&Found| f.manifest.id == generator && f.manifest.spfx_version == version;
    let local = found.iter().position(|f| matches(&f) && f.record.is_none());
    local.or_else(|| found.iter().position(|f| matches(&f)))
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ScaffoldError + '_ {
    move |source| ScaffoldError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Moves `new_dir` to `target`. An existing `target` is set aside first (Windows
/// cannot rename over a folder) and put back if the move fails.
fn swap_into_place(root: &Path, new_dir: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return fs::rename(new_dir, target).map_err(io_err(target));
    }
    let aside = root.join(format!(".old-{}", uuid::Uuid::new_v4()));
    fs::rename(target, &aside).map_err(io_err(target))?;
    match fs::rename(new_dir, target) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&aside);
            Ok(())
        }
        Err(e) => {
            let _ = fs::rename(&aside, target);
            Err(ScaffoldError::Io {
                path: target.to_path_buf(),
                source: e,
            })
        }
    }
}

fn lock_is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map_or(true, |age| age < STALE_LOCK))
        .unwrap_or(false)
}

/// `.update.lock`, removed again when dropped.
struct LockGuard(PathBuf);

impl LockGuard {
    fn acquire(path: &Path) -> Result<Self> {
        for _ in 0..2 {
            match fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => return Ok(Self(path.to_path_buf())),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_fresh(path) {
                        return Err(ScaffoldError::Busy(
                            "Another template update is already running".into(),
                        ));
                    }
                    let _ = fs::remove_file(path); // stale: left over from a crash
                }
                Err(e) => return Err(io_err(path)(e)),
            }
        }
        Err(ScaffoldError::Busy("Could not take the update lock".into()))
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// A scratch folder inside the templates root, removed when dropped.
struct TempFolder(PathBuf);

impl TempFolder {
    fn create(root: &Path) -> Result<Self> {
        let path = root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(io_err(&path))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    const IDX_URL: &str = "https://example.com/index.json";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-store-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    type Served = (Vec<u8>, Option<String>);

    #[derive(Default)]
    struct FakeFetch {
        bodies: RefCell<HashMap<String, Served>>,
        calls: RefCell<Vec<(String, Option<String>)>>,
        offline: Cell<bool>,
    }

    impl FakeFetch {
        fn serve(&self, url: &str, bytes: Vec<u8>, etag: Option<&str>) {
            self.bodies
                .borrow_mut()
                .insert(url.to_string(), (bytes, etag.map(str::to_string)));
        }
        fn call_count(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, etag: Option<&str>) -> Result<FetchResult> {
            self.calls.borrow_mut().push((url.to_string(), etag.map(str::to_string)));
            if self.offline.get() {
                return Err(ScaffoldError::Fetch {
                    url: url.into(),
                    reason: "offline".into(),
                });
            }
            let bodies = self.bodies.borrow();
            let (bytes, tag) = bodies.get(url).ok_or_else(|| ScaffoldError::Fetch {
                url: url.into(),
                reason: "404".into(),
            })?;
            if etag.is_some() && etag == tag.as_deref() {
                return Ok(FetchResult::NotModified);
            }
            Ok(FetchResult::Body {
                bytes: bytes.clone(),
                etag: tag.clone(),
            })
        }
    }

    fn manifest_json(version: &str) -> String {
        format!(
            r#"{{ "id": "spfx-webpart", "spfxVersion": "{version}", "folderFrom": "solutionName",
                 "prompts": [ {{ "key": "solutionName", "type": "text" }} ] }}"#
        )
    }

    /// A `.tar.gz` laid out like a GitHub archive (everything under one top folder).
    fn archive(version: &str, marker: &str, top: Option<&str>) -> Vec<u8> {
        let files = [
            ("manifest.json", manifest_json(version)),
            ("marker.txt", marker.to_string()),
            ("solution/package.json", "{}".to_string()),
        ];
        let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (path, content) in files {
            let full = match top {
                Some(t) => format!("{t}/{path}"),
                None => path.to_string(),
            };
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, full, content.as_bytes()).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn index_json(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let rows: Vec<String> = entries
            .iter()
            .map(|(v, rev, bytes)| {
                format!(
                    r#"{{ "generator": "spfx-webpart", "version": "{v}", "revision": {rev}, "dir": "spfxv.{v}",
                         "url": "https://example.com/spfxv.{v}.tar.gz", "sha256": "{}", "size": {} }}"#,
                    sha256_hex(bytes),
                    bytes.len()
                )
            })
            .collect();
        format!(r#"{{ "schema": 1, "templates": [{}] }}"#, rows.join(",")).into_bytes()
    }

    fn publish(f: &FakeFetch, entries: &[(&str, u32, Vec<u8>)], etag: Option<&str>) {
        let refs: Vec<(&str, u32, &[u8])> = entries.iter().map(|(v, r, b)| (*v, *r, b.as_slice())).collect();
        f.serve(IDX_URL, index_json(&refs), etag);
        for (v, _, bytes) in entries {
            f.serve(&format!("https://example.com/spfxv.{v}.tar.gz"), bytes.clone(), None);
        }
    }

    fn leftovers(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".tmp-") || n.starts_with(".old-") || n == LOCK_FILE)
            .collect()
    }

    const DAY: u64 = 86_400;

    // ------------------------------------------------------------- checking

    #[test]
    fn check_respects_the_interval_etag_and_force() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", Some("repo-1")))], Some("\"v1\""));

        let first = store.check_updates(&f, IDX_URL, 1_000, false, DAY).unwrap();
        assert_eq!(first.status, CheckStatus::Refreshed);
        assert_eq!(f.call_count(), 1);

        // within the interval: no request at all
        let again = store.check_updates(&f, IDX_URL, 1_000 + DAY - 1, false, DAY).unwrap();
        assert_eq!(again.status, CheckStatus::Skipped);
        assert_eq!(f.call_count(), 1);

        // due: conditional request with the ETag, answered "not modified"
        let due = store.check_updates(&f, IDX_URL, 1_000 + DAY, false, DAY).unwrap();
        assert_eq!(due.status, CheckStatus::NotModified);
        assert_eq!(f.calls.borrow()[1].1.as_deref(), Some("\"v1\""));

        // the not-modified answer restarted the interval
        let soon = store.check_updates(&f, IDX_URL, 1_000 + DAY + 10, false, DAY).unwrap();
        assert_eq!(soon.status, CheckStatus::Skipped);

        // "check now" ignores the interval
        let forced = store.check_updates(&f, IDX_URL, 1_000 + DAY + 11, true, DAY).unwrap();
        assert_eq!(forced.status, CheckStatus::NotModified);
        assert_eq!(f.call_count(), 3);
    }

    #[test]
    fn only_https_index_urls_and_valid_indexes_are_accepted() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        assert!(store.check_updates(&f, "http://example.com/i.json", 1, true, DAY).is_err());
        assert_eq!(f.call_count(), 0);

        f.serve(IDX_URL, b"{ \"schema\": 1, \"templates\": [ { \"nope\": 1 } ] }".to_vec(), None);
        assert!(store.check_updates(&f, IDX_URL, 1, true, DAY).is_err());
        assert!(!t.0.join(INDEX_FILE).exists(), "a bad index must not replace the cache");
    }

    #[test]
    fn offline_keeps_the_cached_list_working() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None)), ("1.9.0", 1, archive("1.9.0", "r1", None))], None);
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();

        f.offline.set(true);
        assert!(store.check_updates(&f, IDX_URL, 1 + DAY, false, DAY).is_err());

        let list = store.list_versions("spfx-webpart").unwrap();
        let versions: Vec<&str> = list.versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(versions, ["1.23.2", "1.9.0"]);
        assert!(list.versions.iter().all(|v| v.downloadable && !v.installed));
    }

    #[test]
    fn a_corrupt_cache_is_ignored_not_fatal() {
        let t = TempDir::new();
        fs::write(t.0.join(INDEX_FILE), "{ broken").unwrap();
        fs::write(t.0.join(INSTALLED_FILE), "also broken").unwrap();
        let store = TemplateStore::open(&t.0).unwrap();
        assert!(store.list_versions("spfx-webpart").unwrap().versions.is_empty());
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], None);
        assert_eq!(store.check_updates(&f, IDX_URL, 1, false, DAY).unwrap().status, CheckStatus::Refreshed);
    }

    // ----------------------------------------------------------- installing

    #[test]
    fn update_downloads_verifies_extracts_and_records() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", Some("repo-v1.23.2")))], None);
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();

        let done = store.update(&f, "spfx-webpart", "1.23.2").unwrap();
        assert_eq!(done.revision, 1);
        assert_eq!(done.replaced_revision, None);
        assert_eq!(fs::read_to_string(t.0.join("spfxv.1.23.2/marker.txt")).unwrap(), "r1");
        assert!(t.0.join("spfxv.1.23.2/solution/package.json").is_file());
        assert!(leftovers(&t.0).is_empty(), "{:?}", leftovers(&t.0));

        let r = store.resolve("spfx-webpart", "1.23.2").unwrap();
        assert_eq!(r.revision, Some(1));
        assert!(!r.local);
        assert_eq!(r.manifest.spfx_version, "1.23.2");
        assert_eq!(r.dir, t.0.join("spfxv.1.23.2"));

        let v = &store.list_versions("spfx-webpart").unwrap().versions[0];
        assert!(v.installed && !v.local && !v.update_available && v.downloadable);
        assert_eq!(v.installed_revision, Some(1));
        assert!(store.pending_updates().unwrap().is_empty());
    }

    #[test]
    fn a_new_revision_replaces_the_folder_atomically() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], Some("\"a\""));
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();
        store.update(&f, "spfx-webpart", "1.23.2").unwrap();
        fs::write(t.0.join("spfxv.1.23.2/stale-file.txt"), "from r1 only").unwrap();

        publish(&f, &[("1.23.2", 2, archive("1.23.2", "r2", None))], Some("\"b\""));
        let check = store.check_updates(&f, IDX_URL, 2, true, DAY).unwrap();
        assert_eq!(
            check.updates,
            vec![AvailableUpdate {
                generator: "spfx-webpart".into(),
                version: "1.23.2".into(),
                revision: 2,
                kind: UpdateKind::Revised { from: 1 },
            }]
        );
        let v = &store.list_versions("spfx-webpart").unwrap().versions[0];
        assert!(v.update_available);

        let done = store.update(&f, "spfx-webpart", "1.23.2").unwrap();
        assert_eq!(done.replaced_revision, Some(1));
        assert_eq!(fs::read_to_string(t.0.join("spfxv.1.23.2/marker.txt")).unwrap(), "r2");
        assert!(!t.0.join("spfxv.1.23.2/stale-file.txt").exists(), "old files must not survive");
        assert_eq!(store.resolve("spfx-webpart", "1.23.2").unwrap().revision, Some(2));
        assert!(leftovers(&t.0).is_empty());
    }

    #[test]
    fn a_wrong_checksum_installs_nothing() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], None);
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();
        // the file on the server changes after the index was written
        f.serve("https://example.com/spfxv.1.23.2.tar.gz", archive("1.23.2", "tampered", None), None);

        let err = store.update(&f, "spfx-webpart", "1.23.2").unwrap_err();
        assert!(matches!(err, ScaffoldError::ChecksumMismatch { .. }), "{err}");
        assert!(!t.0.join("spfxv.1.23.2").exists());
        assert!(!t.0.join(INSTALLED_FILE).exists());
        assert!(leftovers(&t.0).is_empty());
    }

    #[test]
    fn the_archive_must_contain_what_the_index_promised() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        // the index says 1.23.2 but the archive is 1.20.0
        let wrong = archive("1.20.0", "r1", None);
        f.serve(IDX_URL, index_json(&[("1.23.2", 1, &wrong)]), None);
        f.serve("https://example.com/spfxv.1.23.2.tar.gz", wrong, None);
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();
        let err = store.update(&f, "spfx-webpart", "1.23.2").unwrap_err().to_string();
        assert!(err.contains("index promised"), "{err}");
        assert!(leftovers(&t.0).is_empty());
    }

    #[test]
    fn local_folders_are_never_overwritten_and_win() {
        let t = TempDir::new();
        // the user's own folder with the same name a download would use
        fs::create_dir_all(t.0.join("spfxv.1.23.2")).unwrap();
        fs::write(t.0.join("spfxv.1.23.2/manifest.json"), manifest_json("1.23.2")).unwrap();
        fs::write(t.0.join("spfxv.1.23.2/marker.txt"), "mine").unwrap();
        // and one with another name for a version that can be downloaded
        fs::create_dir_all(t.0.join("my-dev-1.9.0")).unwrap();
        fs::write(t.0.join("my-dev-1.9.0/manifest.json"), manifest_json("1.9.0")).unwrap();

        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(
            &f,
            &[("1.23.2", 1, archive("1.23.2", "theirs", None)), ("1.9.0", 1, archive("1.9.0", "theirs", None))],
            None,
        );
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();

        let err = store.update(&f, "spfx-webpart", "1.23.2").unwrap_err();
        assert!(matches!(err, ScaffoldError::LocalTemplate(_)), "{err}");
        assert_eq!(fs::read_to_string(t.0.join("spfxv.1.23.2/marker.txt")).unwrap(), "mine");
        assert!(leftovers(&t.0).is_empty());

        let list = store.list_versions("spfx-webpart").unwrap();
        assert!(list.versions.iter().all(|v| v.local && v.installed && !v.downloadable));

        // a download of 1.9.0 may go next to the local one, but the local one still wins
        store.update(&f, "spfx-webpart", "1.9.0").unwrap();
        let r = store.resolve("spfx-webpart", "1.9.0").unwrap();
        assert!(r.local);
        assert_eq!(r.dir, t.0.join("my-dev-1.9.0"));
    }

    #[test]
    fn pending_updates_offer_only_the_newest_new_version() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(
            &f,
            &[("1.23.2", 1, archive("1.23.2", "r1", None)), ("1.9.0", 1, archive("1.9.0", "r1", None))],
            None,
        );
        let check = store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();
        assert_eq!(check.updates.len(), 1);
        assert_eq!(check.updates[0].version, "1.23.2");
        assert_eq!(check.updates[0].kind, UpdateKind::NewVersion);
    }

    // ----------------------------------------------------------- auto update

    #[test]
    fn auto_update_checks_downloads_and_reports() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], None);

        let mut events = Vec::new();
        store
            .auto_update(&f, IDX_URL, UpdateSettings::default(), 1, |e| events.push(e))
            .unwrap();
        assert!(matches!(events[0], UpdateEvent::Checking));
        assert!(matches!(events[1], UpdateEvent::Checked(_)));
        assert!(matches!(events[2], UpdateEvent::Downloading { .. }));
        assert!(matches!(events[3], UpdateEvent::Installed(_)));
        assert!(t.0.join("spfxv.1.23.2/manifest.json").is_file());

        // next start-up within the interval: no network, nothing to do
        let before = f.call_count();
        let mut again = Vec::new();
        store
            .auto_update(&f, IDX_URL, UpdateSettings::default(), 2, |e| again.push(e))
            .unwrap();
        assert_eq!(f.call_count(), before);
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn auto_update_off_only_checks_and_one_failure_does_not_stop_the_rest() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], None);

        let off = UpdateSettings { auto_update: false, ..Default::default() };
        let mut events = Vec::new();
        store.auto_update(&f, IDX_URL, off, 1, |e| events.push(e)).unwrap();
        assert_eq!(events.len(), 2);
        assert!(!t.0.join("spfxv.1.23.2").exists());

        // now on, but the archive is gone from the server
        f.bodies.borrow_mut().remove("https://example.com/spfxv.1.23.2.tar.gz");
        let mut events = Vec::new();
        store
            .auto_update(&f, IDX_URL, UpdateSettings::default(), 1 + DAY, |e| events.push(e))
            .unwrap();
        assert!(events.iter().any(|e| matches!(e, UpdateEvent::Failed { .. })));
        assert!(leftovers(&t.0).is_empty());
    }

    // ------------------------------------------------------------------ lock

    #[test]
    fn a_running_update_blocks_resolve_and_a_second_update_but_a_stale_lock_does_not() {
        let t = TempDir::new();
        let store = TemplateStore::open(&t.0).unwrap();
        let f = FakeFetch::default();
        publish(&f, &[("1.23.2", 1, archive("1.23.2", "r1", None))], None);
        store.check_updates(&f, IDX_URL, 1, true, DAY).unwrap();

        let lock = t.0.join(LOCK_FILE);
        fs::write(&lock, "").unwrap();
        assert!(matches!(store.resolve("spfx-webpart", "1.23.2"), Err(ScaffoldError::Busy(_))));
        assert!(matches!(store.update(&f, "spfx-webpart", "1.23.2"), Err(ScaffoldError::Busy(_))));
        assert!(lock.exists(), "someone else's lock must not be removed");

        // a lock from a crash, long ago
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
        store.update(&f, "spfx-webpart", "1.23.2").unwrap();
        assert!(!lock.exists(), "our own lock is released");
    }

    // --------------------------------------------------------------- archive

    fn raw_entry(name: &str, kind: tar::EntryType, size: u64, link: Option<&str>) -> Vec<u8> {
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_entry_type(kind);
        h.set_size(size);
        h.set_mode(0o644);
        if let Some(l) = link {
            h.as_gnu_mut().unwrap().linkname[..l.len()].copy_from_slice(l.as_bytes());
        }
        h.set_cksum();
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        io::Write::write_all(&mut enc, h.as_bytes()).unwrap();
        enc.finish().unwrap()
    }

    fn extract_err(bytes: &[u8]) -> String {
        let t = TempDir::new();
        crate::archive::extract_tar_gz(bytes, &t.0).unwrap_err().to_string()
    }

    #[test]
    fn unsafe_archives_are_rejected() {
        assert!(extract_err(&raw_entry("evil", tar::EntryType::Symlink, 0, Some("/etc/passwd"))).contains("not allowed"));
        assert!(extract_err(&raw_entry("evil", tar::EntryType::Link, 0, Some("x"))).contains("not allowed"));
        assert!(extract_err(&raw_entry("../escape.txt", tar::EntryType::Regular, 0, None)).contains("Invalid path"));
        assert!(extract_err(&raw_entry("/abs.txt", tar::EntryType::Regular, 0, None)).contains("Invalid path"));
        assert!(extract_err(&raw_entry("big.bin", tar::EntryType::Regular, 21 * 1024 * 1024, None)).contains("larger than"));
        assert!(extract_err(b"this is not gzip").contains("archive"));
    }

    #[test]
    fn an_archive_without_a_manifest_is_rejected() {
        let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "repo/readme.txt", &b"x"[..]).unwrap();
        let bytes = b.into_inner().unwrap().finish().unwrap();
        assert!(extract_err(&bytes).contains("manifest.json was not found"));
    }

    // ------------------------------------------------------------------ root

    #[test]
    fn templates_root_precedence() {
        let env_set = |k: &str| (k == ENV_TEMPLATES_ROOT).then(|| "/from/env".to_string());
        let env_none = |_: &str| None;
        let data = Some(PathBuf::from("/data"));

        let explicit = resolve_root_with(Some(Path::new("/host")), env_set, data.clone()).unwrap();
        assert_eq!(explicit, PathBuf::from("/host"));
        let from_env = resolve_root_with(None, env_set, data.clone()).unwrap();
        assert_eq!(from_env, PathBuf::from("/from/env"));
        let default = resolve_root_with(None, env_none, data).unwrap();
        assert_eq!(default, PathBuf::from("/data/mentenaz/templates"));
        assert!(resolve_root_with(None, env_none, None).is_err());
        let blank = |_: &str| Some("  ".to_string());
        assert_eq!(
            resolve_root_with(None, blank, Some(PathBuf::from("/d"))).unwrap(),
            PathBuf::from("/d/mentenaz/templates")
        );
    }
}
