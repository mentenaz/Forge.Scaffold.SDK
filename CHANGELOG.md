# Changelog

The versions and what counts as a breaking change are in [`docs/api-policy.md`](docs/api-policy.md). While the crate is 0.x, a
minor version may break the supported API.

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
