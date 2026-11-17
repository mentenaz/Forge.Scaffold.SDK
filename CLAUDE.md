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
cargo run --example scaffold -- <generator folder> <location> <name> <webpart>[,<webpart>...]
cargo test --test spfx_generators        # scaffolds every spfxv.* folder of the templates repo, and from a pack of it (skips if absent)
cargo run --example pack -- <generator folder> <revision> <output file>
cargo run --example release -- <out dir> <base url> <generator folder>...   # archives + index.json
```

## What this is

A Rust-native, Yeoman-style scaffolding engine (no Node) used as a git dependency by a Zed/GPUI fork, which supplies the UI panel. Generators are **data**: a `manifest.json` plus template files, read from a folder or from a single compiled-in `.pack.json`. `README.md` and `Overview.md` hold the module table, status and the "not built yet" list (`when` on prompts, `package-name` rule, pack registry, `.mentenaz-template.json` marker, NuGet source).

## Architecture

The pipeline is `Manifest::from_json` → `plan()` → `apply()`:

- `manifest.rs` parses and cross-validates (unknown fields rejected; `when` keys, `packageManagers` and token names checked against prompts; all problems returned together).
- `plan.rs` renders everything **in memory** using `tokens.rs` (`{__token__}` in paths and contents, derived Pascal/Camel/Kebab forms, stable per-scope GUIDs; unknown token = error), expands `include`/`repeat`/`parts`, applies `merge`, and returns `Plan { target, files, warnings, post }`. It writes nothing.
- `apply()` (via `stage.rs`) is the **only writer**: it creates a brand-new `[location]/[name]` folder, refuses if it exists, and removes what it created on IO error.
- Templates are read through the `TemplateSource` trait (`source.rs`): `DirSource` for a folder (symlinks refused), `Pack` (`pack.rs`) for one generator version in a single JSON file with SHA-256-deduped `blobs`. Same generator works from either. `packer.rs` (`pack_dir`) builds a pack from a generator folder and reloads it through `Pack::from_json` before returning. `release.rs` (`build_release`) builds the deterministic `.tar.gz` archives and `index.json` that `store.rs` downloads; hosting is described in `docs/publishing.md`.
- `detect.rs` finds an existing solution on disk (start of "add to existing solution"); `docs/spfx-add-findings.md` is the measured spec of what `yo` changes in that case. `docs/spfx-generator-findings.md` records scaffolding and building all five SPFx generators (1.20.0 to 1.23.2); no engine change was needed.
- Template management: `store.rs` (templates folder, version list, `check_updates`/`update`/`auto_update`, lock file, atomic swap), `index.rs` (remote `index.json` + `installed.json`), `archive.rs` (safe `.tar.gz` extraction), `fetch.rs` (`Fetch` trait).

## Constraints to respect

- The crate is pure logic: it opens no connections and runs no commands. HTTP comes from the host via `Fetch`; `install`/`composeUp` are returned as post-step *descriptors* for the host to run. Don't add network or process execution here.
- Randomness is injected (`SecretSource`, `GuidSource`, with `Sequential*` variants) so tests stay deterministic.
- Folders in the templates root not listed in `installed.json` are user-owned and never overwritten.
- Template/output paths must be relative, no `..`, and no Windows-reserved characters.
- The sibling repo `E:\mentenaz\Forge.Scaffold.Templates` holds the raw captures, the generator folders (`templates/spfxv.<version>/`, made by `tools/make_generator.py`), the schemas and a sample pack whose `base/assets/icon.png` is a truncated placeholder, so it does not load yet. Fix the data, not the engine.
- The crate is `MIT OR Apache-2.0`. The SPFx templates in the sibling repo are derived from Microsoft's generator output and are not covered by that licence; do not publish them until the provenance question (SharePoint/sp-dev-docs discussion 11052) is settled.
