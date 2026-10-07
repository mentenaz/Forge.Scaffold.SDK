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
//! * [`plan`]: `plan()` renders everything in memory, `apply()` writes it
//! * [`secrets`]: generated passwords and secrets
//! * [`fetch`], [`index`], [`archive`], [`store`]: the templates folder, downloads and updates
//!
//! Planned next: the marker file, the NuGet source.

pub mod archive;
pub mod error;
pub mod fetch;
pub mod index;
pub mod manifest;
pub mod plan;
pub mod secrets;
pub mod stage;
pub mod store;
pub mod tokens;
pub mod validate;

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
pub use plan::{apply, plan, Answers, Plan, PlanOptions, PostStepDescriptor, Warning};
pub use secrets::{RandomSecrets, SecretSource, SequentialSecrets};
pub use stage::{StagedFs, WriteOutcome, WriteProgress};
pub use tokens::{render, GuidSource, RandomGuids, SequentialGuids, TokenMap};
pub use validate::{validate_solution_name, validate_webpart_names, ValidationIssue};
