# Changelog

The versions and what counts as a breaking change are in [`docs/api-policy.md`](docs/api-policy.md). While the crate is 0.x, a
minor version may break the supported API.

## 0.4.0-alpha.1 (24 November 2026)

**A first usable slice of service packages. A pre-release: names and fields may still change between alphas, and this file says
when they do.** The panel can build against it; `v0.3.4` stays the release to pin for everything else. How to use it is in
`docs/import-packages.md`; a made-up package of the right shape is `tests/fixtures/acme-data-services`.

### What can be done now

- **Import a package into an SPFx solution** (`plan_import`, `apply_import`, undo with `ImportPlan::undo_record` and
  `revert_edit`): a package is a folder with a `manifest.json` that has an `import` section, a `package.json` that lists what it
  needs, and source. The shared `core` and the chosen `modules` are copied below `src/services/<slug>/` (or `src/components/<slug>/`)
  with their folders; a file that is there and the same is skipped, one that differs is kept and listed; a file named like a
  secret is not copied; the needs are merged into the solution's `package.json`; a second import adds more modules.
- **Say what is already there** (`inspect_import`): per module (and for the core) `Absent`, `Present`, `Modified` or `Partial`,
  with the paths, for a form's switches; and the outcome of each need.
- **Compare the package's needs with the solution's** (`DependencyOutcome`: `Add`, `Skip`, `UpgradeAvailable`, `Clash`,
  `Untouched`, `MissingRequired`, `MissingPeer`; `HaveFrom`: `NodeModules`, `Lock`, `Declared`). Ranges follow npm; what the
  engine cannot tell is a clash saying so. A missing peer or a missing SPFx package stops the import.
- **Warn about an import nothing declares** in the copied code.
- A recipe with no files (`"kind": "library"`) imports only its needs.

### New public names

`plan_import`, `apply_import`, `inspect_import`, `ImportOptions`, `ImportPlan`, `ImportState`, `ModuleState`, `ModuleStatus`,
`DependencyOutcome`, `HaveFrom`, and the manifest's `ImportSpec`, `ImportModule`, `PackageKind` (`Manifest::import`). The enums
are `#[non_exhaustive]`; `ImportOptions` is built with `ImportOptions::new()`. `Manifest` has a new public field, `import`: a
struct literal of `Manifest` needs it (hosts build manifests with `Manifest::from_json`).

### Not built yet (still in the spec)

Upgrades, families (`@pnp/*`), projects that are not SPFx, the authoring checks of a package, a content scan for secrets, the
lock-file warning, multi-select on `list` prompts. `plan_import` takes an `SpfxSolution` for now; it will take a general project
later (a change of signature, announced here).

## 0.3.4 (24 November 2026)

The 1.24 web part templates get a folder without the `WebPart` suffix, and an add refuses a component that is already there under
another folder name. No change to the supported API: the pin can move from `v0.3.3` freely. What the panel sees: an add can now
return `ComponentExists` for a component that is registered under another folder name, and the web part templates released after
the next publish have the new folder layout (nothing to change in the panel).

### Added

- **Overlay `rewrite` rules** (`overlay.json`, and `UpstreamSource::apply_rewrites`, which is release tooling): every occurrence of
  a `from` text in a path and in the text of a file of the named official templates becomes `to`, each rule with a required
  `why`. The templates repo uses one rule: `{componentName.camel}WebPart/` becomes `{componentName.camel}/` in `webpart-react`,
  `webpart-noframework` and `webpart-minimal`, so a web part lands in `src/webparts/requestForm/` (Microsoft's own 1.24
  templates say `requestFormWebPart/`; the 1.20 to 1.23 generators and our house layout say `requestForm/`). The three
  places in `config/config.json` that name the folder follow it; file names (`RequestFormWebPart.ts`, the strings typings) do not
  change. `NOTICE.txt` says how many files were changed this way. The three archives are a new revision; no other archive moves.

### Fixed

- **An add refuses a component whose bundle is already in `config/config.json`,** even when its files are in a folder of
  another layout (`ComponentExists`, with the bundle named). With a solution that has `src/webparts/helloWorldWebPart/` from the
  old layout, adding "Hello World" again would have written `src/webparts/helloWorld/`, found no file in common, kept the existing
  bundle and never built the new folder, and said it had worked.

## 0.3.3 (24 November 2026)

An add-on generator that works on a solution that already exists: **fast-serve** for gulp-based SPFx solutions
(`templates/spfx-fast-serve.1.0.0` in the templates repo; what the official tool does is in `docs/spfx-fast-serve-findings.md`),
and the two things a panel needs to tell such a generator from a template. The pin can move from `v0.3.2` freely: every change is
an addition. Two structs gain a public field, and both are built through constructors, not literals (`AddOptions::new()`, and
`AddPlan` is only ever returned).

### Added

- **`GeneratorInfo::can_create` and `can_add`.** Whether a generator makes a new project (`plan()` takes it) or can be added to
  an existing solution (`plan_add` takes it). `Manifest::can_create()` and `can_add()` say the same for one manifest. From the
  newest folder on disk; for a generator that is only in the index, from two tags (see below). A generator can be both (the SPFx
  ones: a project, and one web part at a time).
- **The index carries it as two tags, `can-create` and `can-add`,** added by the release (`build_release_from`) from each manifest
  (the official SPFx templates: both; create-vite: `can-create`). No new field and no new schema, because **every engine
  released so far (0.2.0 to 0.3.2) refuses an index with a field it does not know (`deny_unknown_fields`, entry and top level)
  and one with a schema above 2**: a schema 3 or an extra field would have broken their template lists and their update checks.
  An older engine shows the two as ordinary tags and reads everything else as before. The new engine takes them out of
  `GeneratorInfo::tags`. An entry published before this version has neither tag; then `can_create` is `true` and `can_add` is
  `false` (what every entry was meant for) until the index is published again. A generator with more than 6 tags of its own
  cannot be released (the index allows 8, and two are added).
- **`AddPlan::post`:** the generator's `post` steps for an add, as for `plan()` (`PostStepDescriptor`). An `install` step is only
  given when the plan changes a `package.json`; nothing to install otherwise. `AddOptions::package_manager` picks the manager
  (default: the generator's own). Before this the `install` question of an add-on did nothing.

- **Merge `package-json-scripts`** (`MergeHelper::PackageJsonScripts`): `package-json`, and also the `scripts` the solution lacks.
  Whatever the solution already has stays, and the plan says which values differed.
- **Merge `spfx-gulpfile`** (`MergeHelper::SpfxGulpfile`): adds the template's `/* name */ ... /* end of name */` blocks in front
  of `build.initialize(`, once, as text.
- **`add.inherit` value `spfxMinor`** (`InheritFrom::SpfxMinor`): the minor part of the solution's SPFx version.
- **`add.anySpfxVersion`**: the check that the generator's `version` is the solution's SPFx minor is skipped.
- A dry run (`check_template`) accepts a generator with no `folderFrom`; `plan()` still refuses it.

## 0.3.2 (24 November 2026)

A fix for a silent failure, with no breaking change. The panel's pin can move from `v0.3.1` freely; two additions to read about are
a new error variant and a new field on `CheckOutcome` (see **Added**).

### Fixed

- **An unreadable `installed.json` is reported, and can no longer do harm.** `installed.json` is the list of the template folders
  this crate downloaded; everything not in it is the user's own and is never overwritten. When it was damaged, empty, or written
  by a newer version, the store read it as an *empty list*, silently: every downloaded template looked like the user's own, updates
  failed with a confusing "folder is not managed" error, and an install would have rewritten the file from the empty list and so
  forgotten every other record. Now:
  - the template lists (`list_versions`, `list_generators`) still work and carry a problem sentence that says what is wrong and
    what to do (restore the file, or delete it with the downloaded folders and check again); the downloaded folders show as the
    user's own meanwhile, since nothing else can be told;
  - a check for updates still refreshes the index, offers **no** updates (nothing can be compared, and offering every template as
    new would be wrong), and carries the same sentence in `CheckOutcome::problems`;
  - what could do harm is **refused before anything is touched**: `pending_updates` and `update` fail with
    `ScaffoldError::InstalledUnreadable`, so the file is never rewritten from an empty list. Nothing is changed. `auto_update`
    downloads nothing (no updates are offered) and the `Checked` event it reports carries the problem;
  - it counts as unreadable when the text is damaged, the file is empty, it has a field this version does not know, or its
    `schema` is newer than this version reads; a file that is absent is a fresh folder, as before;
  - adding the user's own template (`add_local`) is unaffected: it neither reads nor writes `installed.json`.

### Added

- `CheckOutcome::problems: Vec<String>` (empty when all is well) and `ScaffoldError::InstalledUnreadable(String)`, kind
  `installed_unreadable`. Both are non-breaking by the policy (`CheckOutcome` is only received, `ScaffoldError` is
  `#[non_exhaustive]`); if the panel **destructures** `CheckOutcome` with a pattern, add `..`.
- `examples/host_flow.rs` prints the problems of a check.

## 0.3.1 (24 November 2026)

A fix, with no change to the API: nothing a host calls or matches on is different, so the pin can move from `v0.3.0` freely.

### Fixed

- **A downloaded template keeps its title when the index does not have one.** The official SPFx and the create-vite archives hold
  no title in their own manifest (their text lives in the index only, so their archives and revisions do not move), so with
  the cached index missing, or a template no longer listed by the index, `list_generators` showed the bare id
  (`ace-data-visualization` instead of "ACE data visualization"). The store now remembers the title, description and tags the
  index gave when a template is downloaded, and brings them in line with the index at every check for updates (also one that
  is skipped because it was recent, and for templates downloaded before this version). The list reads them last: the user's own
  folder, then the current index, then the folder's own manifest, then what was remembered.
- **Where it is kept, and why.** In a file of its own, `display.json` next to `installed.json`, **not** in `installed.json`.
  An older version of this crate reads `installed.json` strictly and treats a file it cannot read as an empty one, which would make
  every downloaded template look like the user's own and stop it from being updated; it ignores a file it does not know. So a
  host can move the pin back to an older tag without harm, and `installed.json` is byte for byte what it was. The file holds
  nothing that matters: a missing, damaged or odd one is an empty one, text that would not pass the index's own rules is not
  trusted, and it is only written when something changed.

## 0.3.0 (24 November 2026)

Saving a project as a template now works end to end for a panel: the template can carry what a list shows, the report is a plain
value that never holds a file's content, and the store puts the result in the templates folder by itself. Moving the panel's pin
from `v0.2.0` needs one change in your code if you build `CreateOptions` with a struct literal (see **Changed**).

### Changed (breaking)

- **`CreateOptions` has three new public fields**: `title: Option<String>`, `description: Option<String>` and `tags: Vec<String>`.
  Code that builds it with `CreateOptions::new(id, version, replacements)` needs no change (the three are empty there); code that
  builds it with a struct literal must add them (or use `..CreateOptions::new(...)`).
- **`Debug` of `TemplateDraft` and `DraftSource` no longer prints file contents.** It shows the id, the version, the paths and the
  report. Before, `{:?}` of a draft printed every file's bytes, so logging a draft could have written a secret to a log. Nothing
  else reads that output; there is no way to get the contents back from `Debug`, use `DraftSource` and `read_file`.

### Added

- **Display text on a created template.** `CreateOptions::title`, `description` and `tags` are written into the manifest next to the
  id and version, so the template shows in a list as it should. They are checked by the same rules as a manifest and the index (a
  title of at most 80 characters, a description of at most 300, no line breaks, no padding, at most 8 tags of 1 to 30 lowercase
  letters, digits or `-`, none twice) and refused with `ScaffoldError::Validation`, one issue per problem with its field (`title`,
  `description` or `tags`). With none set, the manifest is byte for byte what it was.
- **`TemplateStore::add_local(&draft) -> Result<PathBuf>`** writes a draft into the templates folder as the user's own, in the
  folder `<id>.<version>` (the naming the store's scan expects), and returns it. It never overwrites: it is refused, with the field
  and the reason, for an id that is not lowercase letters, digits and `-`, a version that is not three numbers, an id and version
  that are already in the folder (downloaded or not, under any folder name), a taken folder name, or while an update runs. It writes
  beside the destination and renames into place, so a half-written folder is never listed, and removes what it wrote if a write
  fails. The template is then listed with `local` set (`GeneratorInfo::local`, `VersionInfo::local`, `ResolvedTemplate::local`).
  `TemplateDraft::write_to` stays public.
- **`CreateReport`**, from `TemplateDraft::report()`: a plain value with public fields `files`, `skipped` (`path` and `reason`),
  `counts` (`key`, `text`, `occurrences`, `files`) and `notes`, and `to_json()`. The fields are the contract. It holds paths, reasons,
  names and numbers and **never the contents of a file**: a skipped `.env` is a path and a reason, and a project refused for text
  shaped like a token names the files by path (up to five) and never prints what is in them. A test checks that no secret
  placed in a skipped file, in a kept file or in a refused file reaches the report, the errors, `Debug` or the JSON.
- `TemplateDraft::id()` and `version()`.
- `forge-scaffold create` takes `--title`, `--description` and `--tag` (repeatable), and its result is the same report.
- **Documentation:** every supported item is documented, and the crate denies `missing_docs` so a new one cannot go without.
  Thirteen items have a runnable example that `cargo test` runs: `plan`, `Manifest::visible_prompts`,
  `TemplateStore::list_generators`, `TemplateStore::add_local`, `find_spfx_solutions`, `revert_edit` (an edit and its undo),
  `check_template`, `validate_solution_name`, `suggest_package_name`, `check_node`, `render`, `RemoteIndex::from_json` and
  `ScaffoldError::kind`. The authoring guide (`docs/authoring-templates.md`) covers the display text, the report and `add_local`.

### Unchanged

A token-shaped file still refuses the whole template (the engine cannot write a literal token, and leaving a file out would give a
template that does not reproduce the project). `local` is still exposed on the generator and version listings and on
`ResolvedTemplate`; the "custom" label and the trust notice are the host's.

## 0.2.0 (22 November 2026)

The first version for the Forge panel to pin. The supported API is now exactly what the crate root re-exports, and it is
guarded by a test.

### Changed (breaking)

- **The modules are private.** Everything a host needs is re-exported at the crate root, so `forge_scaffold::plan::plan`
  becomes `forge_scaffold::plan`, `forge_scaffold::manifest::Manifest` becomes `forge_scaffold::Manifest`, and so on. The
  release pipeline (`release`, `upstream`, `create_vite`, `packer`, `archive`) stays `pub` but hidden from the docs and
  outside the supported API.
- **Helpers are crate-private**: the casing helpers of `tokens`, the version and hash helpers of `index`, `when_holds`,
  `validate_range`, and the limit constants (use `Limits::default()` for the limits).
- **The index format is schema 2** (`INDEX_SCHEMA`); schema 1 is still read (`MIN_INDEX_SCHEMA`). An entry can carry `title`,
  `description` and `tags`. `installed.json` keeps its own number.
- The enums a host receives are `#[non_exhaustive]`: match them with a fallback arm.

### Added

- **For the panel's list:** `TemplateStore::list_generators` (title, description, tags, newest version, installed,
  downloadable, sorted by title), and the optional `title`, `description` and `tags` of a manifest.
- **Existing solutions:** `find_spfx_solutions` and `solution_info` (names, type, versions, components), `plan_edit` and
  `apply_edit` (name, description, version), `AddPlan::undo_record` and `revert_edit` (undo with a record the host stores).
- **Template authors:** `check_template` (dry-runs a template in every branch) and `create_template` (a verified template from
  a project), and conditional prompts (`when` on a prompt, `Manifest::visible_prompts`).
- **Safety:** warnings on adds, a warning for files a tool runs on its own, refusal to write through links, configurable
  limits, `ScaffoldError::kind()` and `to_json()`.
- **The `forge-scaffold` program** for hosts that are not written in Rust (`versions`, `form`, `plan`, `apply`, `check`,
  `create`, `info`), whose results carry `protocol` 1.
- CI on Ubuntu, Windows and macOS; `docs/api-policy.md`.

## 0.1.0 (7 October 2026)

The first tag: scaffolding from a folder or a pack, adding SPFx components to a solution, the templates store, releases.
