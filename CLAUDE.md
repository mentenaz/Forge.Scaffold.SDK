# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Windows / PowerShell project, plain Cargo crate (`forge-scaffold`), no workspace.

```powershell
cargo test                      # whole suite (unit tests live inside each src/*.rs)
cargo test plan::               # one module
cargo test <test_name>          # a single test by name substring
cargo clippy --all-targets      # kept clean
cargo run --example detect -- <folder>   # prints what detect_solution finds
cargo run --example add -- <generator folder> <folder in the solution> <web part>[,...] [--apply]   # detect, then add to the solution (exit 3 = not in one)
cargo test --test add_spfx               # adds a web part to a real 1.20.0 solution (needs the templates repo next to this one)
cargo run --example add -- --upstream <spfx repo> <template> <folder in the solution> <component name> [--apply]   # add an official template's component to a heft solution
cargo test --test add_heft               # adds components to real 1.24.1 heft solutions (needs a clone of the SPFx repo as ../spfx or FORGE_UPSTREAM_SPFX)
cargo run --example scaffold -- <generator folder> <location> <name> <webpart>[,<webpart>...]
cargo test --test spfx_generators        # scaffolds every spfxv.* folder of the templates repo, and from a pack of it (skips if absent)
cargo run --example pack -- <generator folder> <revision> <output file>
cargo run --example upstream -- <spfx repo> <template> <location> <component name> [--overlay <folder>]   # official SharePoint/spfx templates
cargo test --test upstream_templates --test upstream_release   # need a clone of the SPFx repo as ../spfx (or FORGE_UPSTREAM_SPFX), the second also the templates repo; skipped otherwise
cargo run --example create_vite -- <vite repo> <template> <location> <project name>   # scaffold a create-vite template (react-ts, vue-ts, ...)
cargo test --test create_vite_templates   # scaffolds all 16 create-vite templates, needs a clone of the Vite repo as ../vite (or FORGE_UPSTREAM_VITE)
cargo test --test own_generators   # scaffolds the backend-api and fullstack generators of the templates repo and checks what the answers change (skips if the repo is absent)
cargo test --test check_real_templates --test create_real_projects   # check_template over all 41 shipped templates, and create_template on generated projects (skip when a repo is absent)
cargo test --test cli --test authoring_example --test hostile_templates   # the JSON program, the guide's worked example, hostile templates
cargo run --bin forge-scaffold -- <versions|form|plan|apply|check|create> ...   # the engine as a JSON program (see the top of src/bin/forge-scaffold.rs)
cargo run --example release -- <out dir> <base url> [--upstream <spfx repo> <template>] [--create-vite <vite repo> <template>] [--overlay <folder>] <generator folder>...   # archives + index.json
```

## What this is

A Rust-native, Yeoman-style scaffolding engine (no Node) used as a git dependency by a Zed/GPUI fork, which supplies the UI panel. Generators are **data**: a `manifest.json` plus template files, read from a folder or from a single compiled-in `.pack.json`. `README.md` and `Overview.md` hold the module table, status and the "not built yet" list (`when` on prompts, `package-name` rule, pack registry, `.mentenaz-template.json` marker, NuGet source).

## Public enums

The enums a host receives from the engine and may match on (`Warning`, `ScaffoldError`, `PostAction`, `PromptKind`, `NameRule`,
`MergeStrategy`, `MergeHelper`, `PackageManager`, `BuildSystem`, `NodeCheck`, `AddOutcome`, `CheckStatus`, `UpdateKind`,
`UpdateEvent`, ...) are `#[non_exhaustive]`: adding a variant is not a breaking change, and anything outside the crate (the examples,
`src/bin`, the integration tests) must have a fallback arm. Enums a host only constructs (`FetchResult`, `ReleaseInput`) are not.

## Public API

What `src/lib.rs` re-exports at the crate root without `#[doc(hidden)]` is the supported API; the modules are private, so use
the root path (`forge_scaffold::plan`, not `forge_scaffold::plan::plan`) in examples, tests and `src/bin`. The release pipeline
(`release`, `upstream`, `create_vite`, `packer`, `archive`) is `pub` but hidden, and is tooling, not API. New helpers are
`pub(crate)` unless a host needs them. `tests/public_api.rs` imports every supported name: when you add or remove one on
purpose, change it there and in `CHANGELOG.md`. Every supported item needs a doc comment (`#![deny(missing_docs)]`), and
a new entry point deserves a runnable example in its docs: they run as doctests. The versions, what is breaking, the JSON format numbers and the release steps
are in `docs/api-policy.md`; a release needs `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` to be clean too.

## CI

`.github/workflows/ci.yml` runs `cargo test --locked` and `cargo clippy --all-targets -- -D warnings` on Ubuntu, Windows and
macOS for every push to `main`, every pull request and by hand. The real-template suites run against the forks `mentenaz/spfx` and `mentenaz/vite` and against the templates repo
`mentenaz/Forge.Scaffold.Templates` (all public; sparse checkouts next to the engine, as the tests expect). CI keeps Unix line
endings on Windows (`core.autocrlf false`) and sets `FORGE_REQUIRE_UPSTREAM=1`, which makes a real-template suite fail instead of
skipping when one of them is missing. If the templates repo is made private again, add the secret `CHECKOUT_TOKEN` (a
fine-grained token with read access to its contents). Before pushing, `cargo check --all-targets --target x86_64-unknown-linux-gnu` and `--target aarch64-apple-darwin`
catch platform-only code (the `#[cfg(unix)]` blocks) without a Linux machine.

## Architecture

The pipeline is `Manifest::from_json` → `plan()` → `apply()`:

- `manifest.rs` parses and cross-validates (unknown fields rejected; `when` keys, `packageManagers` and token names checked against prompts; all problems returned together). A text prompt can carry `rule: solution-name` or `package-name` (npm names the way create-vite checks them, `validate.rs`), and an `include` or `repeat` can `rename` plain file names while copying (`{ "_gitignore": ".gitignore" }`, applied in `copy_folder`).
- `plan.rs` renders everything **in memory** using `tokens.rs` (`{__token__}` in paths and contents, derived Pascal/Camel/Kebab forms, stable per-scope GUIDs; unknown token = error), expands `include`/`repeat`/`parts`, applies `merge`, and returns `Plan { target, files, warnings, post }`. It writes nothing.
- `apply()` (via `stage.rs`) is the only writer of a **new** solution: it creates a brand-new `[location]/[name]` folder, refuses if it exists, and removes what it created on IO error. `apply_add()` (`add.rs`) is the only writer into an **existing** solution: it never deletes or overwrites, only creates files and edits some through a merge, checks nothing changed since `plan_add()`, and restores everything on a failed write.
- Templates are read through the `TemplateSource` trait (`source.rs`): `DirSource` for a folder (symlinks refused), `Pack` (`pack.rs`) for one generator version in a single JSON file with SHA-256-deduped `blobs`. Same generator works from either. `packer.rs` (`pack_dir`) builds a pack from a generator folder and reloads it through `Pack::from_json` before returning. `upstream.rs` reads the official SharePoint/spfx template format (EJS `<%= %>` tags, `{x.camel}` paths) by translating it once into `{__token__}`s, so those templates run through the same pipeline; `docs/upstream-spfx-findings.md` has the results. `release.rs` (`build_release_from`) takes generator folders or upstream templates (converted at release time, with an optional `Overlay` for your own images) and builds the deterministic `.tar.gz` archives and `index.json` that `store.rs` downloads; hosting is described in `docs/publishing.md`.
- `detect.rs` finds an existing solution on disk: `detect_spfx` reads it from its files (the marker `.yo-rc.json` is only a hint, the official CLI does not write it), and the host asks it first: a solution means `plan_add()`/`apply_add()`, none means `plan()`/`apply()`. How a generator is added is its manifest's `add.mode`: `items` runs only its `repeat` folders and `per` merges (our generators), `render` renders the whole template and writes it file by file (the official templates), with the four add-only merges in `merge.rs` (`package.json`, `config/config.json`, `package-solution.json`, `serve.json`) that keep the file's formatting and never change a value the solution has; `jsonc.rs` edits JSON with comments/trailing commas as text so they survive a merge (blank-with-spaces keeps byte offsets); `node_range.rs` checks `engines.node` ranges for `Manifest::node_check` and `SpfxSolution::node_check`; `examples/host_flow.rs` is the whole host flow; `check.rs` (`check_template`: dry-run a template in every branch, `plan_dry`), `create.rs` (`create_template`: a project into a verified template) and `src/bin/forge-scaffold.rs` (the JSON program for non-Rust hosts) are the template tools, see `docs/authoring-templates.md`; `docs/add-to-solution.md`; `docs/spfx-add-findings.md` is the measured spec of what `yo` changes in that case. `docs/spfx-generator-findings.md` records scaffolding and building all five SPFx generators (1.20.0 to 1.23.2); no engine change was needed.
- Template management: `store.rs` (templates folder, version list, `check_updates`/`update`/`auto_update`, lock file, atomic swap), `index.rs` (remote `index.json` + `installed.json`), `archive.rs` (safe `.tar.gz` extraction), `fetch.rs` (`Fetch` trait).

## Constraints to respect

- The crate is pure logic: it opens no connections and runs no commands. HTTP comes from the host via `Fetch`; `install`/`composeUp` are returned as post-step *descriptors* for the host to run. Don't add network or process execution here.
- Randomness is injected (`SecretSource`, `GuidSource`, with `Sequential*` variants) so tests stay deterministic.
- Folders in the templates root not listed in `installed.json` are user-owned and never overwritten. A custom template is trusted by the user and Forge is not responsible for what it does: `docs/custom-templates.md` is the statement and the list of protections (`tests/hostile_templates.rs`). Keep both true when changing path handling, limits or warnings.
- Template/output paths must be relative, no `..`, and no Windows-reserved characters.
- The sibling repo `E:\mentenaz\Forge.Scaffold.Templates` holds the raw captures, the generator folders (`templates/spfxv.<version>/`, made by `tools/make_generator.py`), the schemas and a sample pack whose `base/assets/icon.png` is a truncated placeholder, so it does not load yet. Fix the data, not the engine.
- Releases of official SPFx templates always put a `NOTICE.txt` with the SPFx repo's `LICENSE` text in the archive (MIT requires the notice to travel with copies) and are refused without that file.
- The crate is `MIT OR Apache-2.0`. The `yo`-derived SPFx generators in the sibling repo are derived from Microsoft's generator output and are not covered by that licence; the repo owner has chosen to publish them while the provenance question (SharePoint/sp-dev-docs discussion 11052) is open, so do not describe them as licensed or cleared.
