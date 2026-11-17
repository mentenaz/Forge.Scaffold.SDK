//! # forge-scaffold
//!
//! Rust-native scaffolding engine (Yeoman-style, no Node).
//!
//! This first slice contains the pure building blocks, all testable without
//! network access:
//!
//! * [`manifest`]: the generator manifest (prompts, repeat, merge) as data
//! * [`tokens`]: `{__token__}` rendering, derived casings, GUID tokens
//! * [`stage`]: in-memory staged filesystem and the safe "new folder only" writer
//! * [`validate`]: name validation for the solution and web parts
//!
//! * [`detect`]: finding an existing solution on disk (the start of "add to an existing solution")
//! * [`source`], [`pack`]: where template files come from (a folder, or a pack held in memory)
//! * [`packer`]: turns a generator folder into a pack
//! * [`release`]: turns generator folders into `.tar.gz` archives plus the `index.json` the store downloads
//! * [`mod@plan`]: `plan()` renders everything in memory, `apply()` writes it
//! * [`secrets`]: generated passwords and secrets
//! * [`fetch`], [`index`], [`archive`], [`store`]: the templates folder, downloads and updates
//!
//! Planned next: the `.mentenaz-template.json` marker file, the pack registry, and the NuGet source. `when` on prompts and the
//! `package-name` rule are also still open. See `README.md`.

pub mod archive;
pub mod detect;
pub mod error;
pub mod fetch;
pub mod index;
pub mod manifest;
pub mod pack;
pub mod packer;
pub mod plan;
pub mod release;
pub mod secrets;
pub mod source;
pub mod stage;
pub mod store;
pub mod tokens;
pub mod validate;

pub use detect::{detect_solution, BuildSystem, DetectRule, Solution};
pub use error::{Result, ScaffoldError};
pub use fetch::{Fetch, FetchResult};
pub use index::{CachedIndex, IndexEntry, InstalledFile, InstalledRecord, RemoteIndex};
pub use store::{
    resolve_templates_root, unix_now, AvailableUpdate, CheckOutcome, CheckStatus, InstallOutcome,
    ResolvedTemplate, TemplateStore, UpdateEvent, UpdateKind, UpdateSettings, VersionInfo,
    VersionList,
};
pub use manifest::{
    Include, Manifest, Merge, MergeStrategy, NameRule, PackageManager, PackageManagers, Part,
    PostAction, PostStep, Prompt, PromptKind, Repeat, When,
};
pub use pack::{Pack, PACK_SCHEMA};
pub use packer::{pack_dir, PackOptions, DEFAULT_EXCLUDES};
pub use release::{build_archive, build_release, Release, ReleaseArchive, ReleaseOptions};
pub use plan::{apply, plan, Answers, Plan, PlanOptions, PostStepDescriptor, Warning};
pub use secrets::{RandomSecrets, SecretSource, SequentialSecrets};
pub use source::{DirSource, TemplateSource};
pub use stage::{StagedFs, WriteOutcome, WriteProgress};
pub use tokens::{render, GuidSource, RandomGuids, SequentialGuids, TokenMap};
pub use validate::{validate_solution_name, validate_webpart_names, ValidationIssue};
