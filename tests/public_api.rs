//! The supported API, as a guard: every name a host may depend on is imported here, so removing or renaming one is a
//! compile error in this test and not a surprise in the Forge panel. When a name is removed on purpose, remove it here in
//! the same change and say so in `CHANGELOG.md` (a breaking change, so a new minor version while the crate is 0.x).
//!
//! The list is what `src/lib.rs` re-exports at the crate root without `#[doc(hidden)]`.

#[allow(unused_imports)]
mod supported {
    pub use forge_scaffold::{
        AddBuildSystem, AddFile, AddMode, AddOptions, AddOutcome, AddPlan, AddSpec, Answers, AvailableUpdate,
        BuildSystem, CachedIndex, CheckOptions, CheckOutcome, CheckStatus, ComponentInfo, ComponentKind,
        CreateOptions, CreateReport, DetectRule, DirSource, DraftSource, ENV_TEMPLATES_ROOT, Fetch, FetchResult,
        GeneratorInfo, GeneratorList, GuidSource, INDEX_SCHEMA, Include, IndexEntry, InheritFrom, InstallOutcome,
        InstalledFile, InstalledRecord, Limits, MIN_INDEX_SCHEMA, Manifest, Merge, MergeHelper, MergeStrategy,
        NameRule, NodeCheck, PACK_SCHEMA, Pack, PackLimits, PackageManager, PackageManagers, Part, Plan, PlanOptions,
        PostAction, PostStep, PostStepDescriptor, Problem, Prompt, PromptKind, RandomGuids, RandomSecrets,
        RemoteIndex, Repeat, Replacement, ReplacementCount, Requires, ResolvedTemplate, Result, RevertOutcome,
        ScaffoldError, SecretSource, SequentialGuids, SequentialSecrets, Skipped, Solution, SolutionEdit,
        SolutionInfo, SpfxSolution, StagedFs, TemplateDraft, TemplateReport, TemplateSource, TemplateStore, TokenMap,
        UNDO_FORMAT, UndoEntry, UndoRecord, UpdateEvent, UpdateKind, UpdateSettings, ValidationIssue, Variant,
        VersionInfo, VersionList, Warning, When, WorkspaceScan, WriteOutcome, WriteProgress, apply, apply_add,
        apply_edit, check_node, check_template, check_with_manifest, create_template, detect_solution, detect_spfx,
        find_spfx_solutions, plan, plan_add, plan_edit, render, resolve_templates_root, revert_edit, solution_info,
        suggest_package_name, unix_now, validate_package_name, validate_solution_name, validate_webpart_names,
    };
}

#[test]
fn the_supported_names_are_all_there_and_the_formats_have_their_numbers() {
    // the numbers of the JSON the crate reads and writes: a change here is a change of format
    assert_eq!(forge_scaffold::INDEX_SCHEMA, 2);
    assert_eq!(forge_scaffold::MIN_INDEX_SCHEMA, 1);
    assert_eq!(forge_scaffold::PACK_SCHEMA, 1);
    assert_eq!(forge_scaffold::UNDO_FORMAT, 1);
    // the crate's own version is the one the changelog and the tag talk about
    assert!(env!("CARGO_PKG_VERSION").starts_with("0."), "a 1.0 needs a deliberate decision, not a bump");
}
