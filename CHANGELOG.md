# Changelog

The versions and what counts as a breaking change are in [`docs/api-policy.md`](docs/api-policy.md). While the crate is 0.x, a
minor version may break the supported API.

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
