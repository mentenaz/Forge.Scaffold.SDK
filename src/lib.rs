//! # forge-scaffold
//!
//! A Rust-native scaffolding engine (Yeoman-style, no Node). Generators are data: a `manifest.json` plus template files, read
//! from a folder, from a downloaded archive, or from a pack held in memory. The crate is pure logic: it opens no connections
//! and runs no commands; a host (the Forge panel) supplies the HTTP through [`Fetch`] and runs the follow-up steps it is
//! handed as data.
//!
//! ## The supported API
//!
//! **What is re-exported at the crate root is the API a host may depend on.** The modules are private on purpose, so the
//! root is the only path: `forge_scaffold::plan`, `forge_scaffold::TemplateStore`, and so on. In the order a host meets them:
//!
//! * **Templates folder and updates:** [`TemplateStore`] (`list_generators`, `list_versions`, `resolve`, `check_updates`,
//!   `update`), [`RemoteIndex`] and its entries (schema 2: title, description, tags), [`Fetch`]
//! * **A new project:** [`Manifest`] and its prompts (`visible_prompts`), [`plan()`] then [`apply()`], with [`PlanOptions`],
//!   [`Answers`], [`Warning`] and [`PostStepDescriptor`]; [`Limits`] for the size limits
//! * **An existing SPFx solution:** [`find_spfx_solutions`] and [`solution_info`] (what to show), [`detect_spfx`], [`plan_add`]
//!   then [`apply_add`], [`plan_edit`] then [`apply_edit`] (name, description, version), [`AddPlan::undo_record`] and
//!   [`revert_edit`] (undo), [`check_node`]
//! * **Template authors:** [`check_template`] (dry-run a template in every branch) and [`create_template`] (make a template
//!   from a project)
//! * **Sources and packs:** [`DirSource`], [`TemplateSource`], [`Pack`]
//! * **Errors:** [`ScaffoldError`] and [`Result`]. The enums a host receives are `#[non_exhaustive]`: match them with a
//!   fallback arm.
//!
//! The release pipeline (`build_release`, the SPFx and create-vite readers, the pack builder) is tooling for the templates
//! repository. It is public so the examples and that repository can use it, and it is **hidden from the documentation and
//! not part of the supported API**: it can change in any release.
//!
//! ## Versioning
//!
//! Semantic versioning, read the way 0.x asks for: a **minor** version (0.2, 0.3) may break the supported API and a **patch**
//! version never does. Every release is tagged (`v0.2.0`) and has an entry in `CHANGELOG.md`. The JSON the crate and the
//! `forge-scaffold` program exchange carries its own number, separate from the crate's: the index `schema`, the pack
//! `schema`, the undo record `format` and the program's `protocol`. Adding a field never changes a number; removing or
//! renaming one, or changing what it means, does. `docs/api-policy.md` has the full policy.
//!
//! **Pending dependency: the analysis engine** (`forge-analysis`, a separate project). Until it is finished, tested and
//! confirmed to find what `docs/integration-notes.md` lists, the template engine only marks where it will plug in, with a
//! comment tagged `TODO(forge-analysis)` (search for the tag). Nothing in this crate calls it yet, and nothing here panics for
//! want of it: the marks are comments, not `todo!()`.
//!

mod add;
mod check;
mod create;
#[doc(hidden)]
pub mod create_vite;
#[doc(hidden)]
pub mod archive;
mod detect;
mod edit;
mod error;
mod fetch;
mod info;
mod index;
mod manifest;
mod node_range;
mod jsonc;
mod merge;
mod pack;
#[doc(hidden)]
pub mod packer;
mod plan;
#[doc(hidden)]
pub mod release;
mod secrets;
mod source;
mod stage;
mod store;
mod tokens;
mod undo;
#[doc(hidden)]
pub mod upstream;
mod validate;

pub use add::{apply_add, plan_add, AddFile, AddOptions, AddOutcome, AddPlan};
pub use detect::{detect_solution, detect_spfx, BuildSystem, DetectRule, Solution, SpfxSolution};
pub use edit::{apply_edit, plan_edit, SolutionEdit};
pub use undo::{revert_edit, RevertOutcome, UndoEntry, UndoRecord, UNDO_FORMAT};
pub use info::{find_spfx_solutions, solution_info, ComponentInfo, ComponentKind, SolutionInfo, WorkspaceScan};
pub use error::{Result, ScaffoldError};
pub use fetch::{Fetch, FetchResult};
pub use index::{CachedIndex, IndexEntry, InstalledFile, InstalledRecord, RemoteIndex, INDEX_SCHEMA, MIN_INDEX_SCHEMA};
pub use store::{
    GeneratorInfo, GeneratorList,
    resolve_templates_root, unix_now, ENV_TEMPLATES_ROOT, AvailableUpdate, CheckOutcome, CheckStatus, InstallOutcome,
    ResolvedTemplate, TemplateStore, UpdateEvent, UpdateKind, UpdateSettings, VersionInfo,
    VersionList,
};
pub use node_range::{check_node, NodeCheck};
pub use manifest::{
    AddBuildSystem, AddMode, AddSpec, InheritFrom, MergeHelper, Include, Requires, Manifest, Merge, MergeStrategy, NameRule, PackageManager, PackageManagers, Part,
    PostAction, PostStep, Prompt, PromptKind, Repeat, When,
};
pub use pack::{Pack, PackLimits, PACK_SCHEMA};
#[doc(hidden)]
pub use packer::{pack_dir, PackOptions, DEFAULT_EXCLUDES};
pub use check::{check_template, check_with_manifest, CheckOptions, Problem, TemplateReport, Variant};
pub use create::{create_template, CreateOptions, DraftSource, Replacement, ReplacementCount, Skipped, TemplateDraft};
#[doc(hidden)]
pub use create_vite::{ViteSource, ViteTemplate, CREATE_VITE_DIR};
#[doc(hidden)]
pub use upstream::{Overlay, UpstreamInputs, UpstreamSource, UpstreamTemplate, TEMPLATE_FOLDER};
#[doc(hidden)]
pub use release::{
    build_archive, build_release, build_release_from, Release, ReleaseArchive, ReleaseInput, ReleaseOptions,
};
pub use plan::{apply, plan, Answers, Plan, PlanOptions, PostStepDescriptor, Warning};
pub use secrets::{RandomSecrets, SecretSource, SequentialSecrets};
pub use source::{DirSource, TemplateSource};
pub use stage::{Limits, StagedFs, WriteOutcome, WriteProgress};
pub use tokens::{render, GuidSource, RandomGuids, SequentialGuids, TokenMap};
pub use validate::{suggest_package_name, validate_package_name, validate_solution_name, validate_webpart_names, ValidationIssue};
