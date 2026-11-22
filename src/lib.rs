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
//! * [`detect`]: finding an existing solution on disk (`detect_spfx` reads it from its files)
//! * [`add`]: adding components to that solution: `plan_add` shows what would change, `apply_add` writes it
//! * [`source`], [`pack`]: where template files come from (a folder, or a pack held in memory)
//! * [`packer`]: turns a generator folder into a pack
//! * [`check`]: checks a template folder before it is used: loads the manifest and dry-runs the plan with sample answers
//! * [`create`]: makes a template from an existing project ("save this project as a template"), verified by scaffolding it back
//! * [`create_vite`]: reads the templates of create-vite (the Vite scaffolder) and does what it does to them
//! * [`upstream`]: reads the template format of the official SharePoint/spfx CLI (EJS tags and `{x.camel}` paths)
//! * [`release`]: turns generator folders into `.tar.gz` archives plus the `index.json` the store downloads
//! * [`mod@plan`]: `plan()` renders everything in memory, `apply()` writes it
//! * [`secrets`]: generated passwords and secrets
//! * [`fetch`], [`index`], [`archive`], [`store`]: the templates folder, downloads and updates
//!
//! Planned next: the `.mentenaz-template.json` marker file, the pack registry, and the NuGet source. `when` on prompts is
//! also still open. See `README.md`.
//!
//! **Pending dependency: the analysis engine** (`forge-analysis`, a separate project). Until it is finished, tested and
//! confirmed to find what `docs/integration-notes.md` lists, the template engine only marks where it will plug in, with a
//! comment tagged `TODO(forge-analysis)` (search for the tag). Nothing in this crate calls it yet, and nothing here panics for
//! want of it: the marks are comments, not `todo!()`.
//!

pub mod add;
pub mod check;
pub mod create;
pub mod create_vite;
pub mod archive;
pub mod detect;
pub mod error;
pub mod fetch;
pub mod info;
pub mod index;
pub mod manifest;
pub mod node_range;
mod jsonc;
mod merge;
pub mod pack;
pub mod packer;
pub mod plan;
pub mod release;
pub mod secrets;
pub mod source;
pub mod stage;
pub mod store;
pub mod tokens;
pub mod upstream;
pub mod validate;

pub use add::{apply_add, plan_add, AddFile, AddOptions, AddOutcome, AddPlan};
pub use detect::{detect_solution, detect_spfx, BuildSystem, DetectRule, Solution, SpfxSolution};
pub use info::{find_spfx_solutions, solution_info, ComponentInfo, ComponentKind, SolutionInfo, WorkspaceScan};
pub use error::{Result, ScaffoldError};
pub use fetch::{Fetch, FetchResult};
pub use index::{CachedIndex, IndexEntry, InstalledFile, InstalledRecord, RemoteIndex};
pub use store::{
    resolve_templates_root, unix_now, AvailableUpdate, CheckOutcome, CheckStatus, InstallOutcome,
    ResolvedTemplate, TemplateStore, UpdateEvent, UpdateKind, UpdateSettings, VersionInfo,
    VersionList,
};
pub use node_range::{check_node, NodeCheck};
pub use manifest::{
    AddBuildSystem, AddMode, AddSpec, InheritFrom, MergeHelper, Include, Requires, Manifest, Merge, MergeStrategy, NameRule, PackageManager, PackageManagers, Part,
    PostAction, PostStep, Prompt, PromptKind, Repeat, When,
};
pub use pack::{Pack, PACK_SCHEMA};
pub use packer::{pack_dir, PackOptions, DEFAULT_EXCLUDES};
pub use check::{check_template, check_with_manifest, CheckOptions, Problem, TemplateReport, Variant};
pub use create::{create_template, CreateOptions, DraftSource, Replacement, ReplacementCount, Skipped, TemplateDraft};
pub use create_vite::{ViteSource, ViteTemplate, CREATE_VITE_DIR};
pub use upstream::{Overlay, UpstreamInputs, UpstreamSource, UpstreamTemplate, TEMPLATE_FOLDER};
pub use release::{
    build_archive, build_release, build_release_from, Release, ReleaseArchive, ReleaseInput, ReleaseOptions,
};
pub use plan::{apply, plan, Answers, Plan, PlanOptions, PostStepDescriptor, Warning};
pub use secrets::{RandomSecrets, SecretSource, SequentialSecrets};
pub use source::{DirSource, TemplateSource};
pub use stage::{Limits, StagedFs, WriteOutcome, WriteProgress};
pub use tokens::{render, GuidSource, RandomGuids, SequentialGuids, TokenMap};
pub use validate::{suggest_package_name, validate_package_name, validate_solution_name, validate_webpart_names, ValidationIssue};
