# Forge.Scaffold

Rust-native scaffolding engine (Yeoman-style, no Node). Generators are data: a manifest plus template files, read from a folder or from a pack compiled into the app.

## Status

All pure logic. The crate opens no connections itself, the host supplies HTTP through the `Fetch` trait. 259 unit tests plus the integration tests, passing on Windows.

| Module | What it does |
|---|---|
| `manifest` | Generator manifest as data: prompts (`text`, `bool`, `list`, `choice`, `sensitive`, name `rule`), `include`, `repeat`, `parts` (ordered fragments), `merge` (`value` or `valueFile`), `secrets`, `post` steps, `when` (`equals`, `in`, `all`), `packageManagers` |
| `source` | The `TemplateSource` trait ("list the files below a folder", "read a file") that `plan()` reads templates through, plus `DirSource` for a real folder (symlinks refused) |
| `pack` | `Pack::from_json`: one generator version in a single file (manifest + every template file, text or base64). A file's content is inline or stored once in `blobs` under its SHA-256 and referenced by that hash, so a file used in many variants costs its size once. Loading checks the schema version, safe and unique paths, every hash, size limits (`PackLimits`), and that the manifest only refers to folders and files that exist. A `Pack` is a `TemplateSource`. Format: `pack.v1.json` |
| `packer` | `pack_dir`: a generator folder in, one `pack.v1.json` out. Text as `utf8`, everything else as `base64`, repeated content stored once in `blobs`, deterministic output, optional newline normalization, a configurable exclude list. The result is loaded back with `Pack::from_json` before it is returned |
| `release` | `build_release` / `build_release_from`: generator folders (or official SPFx templates, with an optional overlay of your own images) in, deterministic `.tar.gz` archives plus the `index.json` the store downloads. Same folder gives the same bytes, so `sha256` only changes with content. `revision` is kept when an archive is unchanged against the previous index and bumped when it is not. Extractor limits are checked while building. Every archive of an official template carries a `NOTICE.txt` with Microsoft's MIT notice and licence text, and the release is refused without it |
| `check` | `check_template` loads a template folder (or pack, or converted upstream template), validates its manifest and dry-runs the plan with sample answers, once for the defaults and once for every other value of every yes/no and choice prompt. It reports problems, notes (an unused prompt, a file nothing copies) and each variant's warnings, and writes nothing. All 41 templates we ship check clean |
| `create` | `create_template` makes a template from an existing project: the texts you name become tokens in contents and names, secrets, dependency folders and links are left out, and the draft is verified by scaffolding it back and comparing with the project byte for byte. Tried on a generated Vite project and an SPFx web part. See `docs/authoring-templates.md` |
| `forge-scaffold` (the program) | The engine as a command that speaks JSON, for hosts not written in Rust: `versions`, `form`, `plan`, `apply` (progress lines), `check`, `create`. It runs nothing: the follow-up steps come back as data |
| `create_vite` | Reads the templates of `create-vite` (`packages/create-vite` of `vitejs/vite`, MIT) and does what the scaffolder does to them: the project name in `package.json` and the page title, `_gitignore` and `_oxlintrc.json` written as dotfiles (the manifest's `rename`), the name checked as an npm package name. All 16 templates scaffold as the template folder with exactly those changes, and `react-ts` installs and builds. Release-time conversion like `upstream`, with create-vite's licence in every archive; what create-vite does with code (the ESLint and React Compiler options, the `custom-*` hand-overs) is not done. See `docs/vite-findings.md` |
| `upstream` | Reads the template format of the official `SharePoint/spfx` CLI (MIT, Microsoft): `template.json`, `<%= name.casing %>` tags and `{name.casing}` paths. Tags are translated once into `{__token__}`s, anything beyond plain substitution is an error, `_phase:*` scripts are stripped, and the rest is the normal pipeline. All 20 upstream templates scaffold, install, build and package, and `webpart-react` matches Microsoft's own EJS output byte for byte. An `Overlay` (`overlay.json`) replaces images such as the welcome pictures and Teams icons with your own |
| `plan` | `plan()` renders everything in memory and returns files, warnings (nested solution, long path, untested package manager) and post-step *descriptors*. `apply()` is the only writer |
| `detect` | `detect_spfx` recognises an SPFx solution from its own files (`package.json`, `config/`, the `@microsoft/sp-*` packages): version, gulp or heft, React. `.yo-rc.json` is only a hint, because the official CLI does not write it. This answers "add to it, or scaffold a new one?" |
| `add` | `plan_add` shows what adding a component to a detected solution would change; `apply_add` writes it, refuses to overwrite, checks nothing changed since the plan, and restores everything if a write fails. A manifest's `add.mode` picks how: `items` (our generators: the per-web-part folders and the `config.json` entries) or `render` (the official templates: the whole template is rendered and written file by file, new files created, identical ones skipped, `package.json` and `config/*.json` merged by four add-only helpers that keep indent, line endings and key order, anything else that differs kept and reported). Verified on SPFx 1.20.0 (gulp) and 1.24.1 (heft), see `docs/add-to-solution.md` |
| `fetch` | The `Fetch` trait (HTTP GET with ETag) the host implements |
| `index` | The remote `index.json` format (`sha256`, `revision`, https-only URLs) plus the local cache and `installed.json` |
| `archive` | Safe `.tar.gz` extraction: files and folders only, path checks, caps on file count and size |
| `store` | The templates folder: version list (newest first, works offline), `resolve`, `check_updates` (24 h interval, ETag, force), `update`, `auto_update` with events, lock file, atomic swap |
| `secrets` | Generated alphanumeric secrets (32 chars, always upper + lower + digit) |
| `tokens` | `{__token__}` rendering on paths and content, derived forms (`Pascal`, `Camel`, `Kebab`), stable per-scope GUID tokens |
| `stage` | In-memory staged filesystem, case-insensitive duplicate and file/folder conflict detection, "new folder only" writer with rollback |
| `validate` | Name validation for the solution, web parts and npm package names (`validate_package_name`, plus `suggest_package_name` to pre-fill a field), returned as data for the panel |

## How a run works

```text
manifest.json  ──► Manifest::from_json   parse + cross-validate (typos rejected)
                      │
answers (panel) ──────┤
                      ▼
templates (folder | pack) ─► plan()   render in memory → Plan { target, files, warnings, post }
                                          │
                                          ▼
                                   apply()   the only writer → new folder, rolled back on error
```

1. **`Manifest::from_json`** — parses and cross-checks the manifest (`when` keys and `packageManagers` against the prompts, path safety, token names). All problems come back together.
2. **`plan(manifest, templates, answers, options)`** — resolves defaults, validates names, renders every `{__token__}`, expands `include`/`repeat`/`parts`, applies `merge`, then reports warnings and the post steps the host should run. Writes nothing.
3. **`apply(&plan, on_progress)`** — creates `[location]/[folderFrom answer]` and writes. Refuses an existing folder; removes the folder it created if an IO error happens halfway.

## Using a pack compiled into the app

```rust
let pack = Pack::from_json(include_str!("spfx-webpart-1.23.2-r1.pack.json"))?;
let planned = plan(pack.manifest(), &pack, &answers, PlanOptions::new(location))?;
apply(&planned, |_| {})?;
```

`manifest.json` may start with a `"$schema"` line (ignored by the engine) so editors can validate it against `manifest.v1.json`.

The crate never runs commands. `install` and `composeUp` come back as descriptors for the host's script runner.

## Custom templates, and who is responsible

A user can put their own generator folder in the templates folder: it is never overwritten and it wins over a download. A custom
template is trusted by whoever uses it, and Forge is not responsible for damage it causes. `docs/custom-templates.md` says so,
lists what the engine does to limit harm (paths are checked after the tokens are filled in, links are refused, size limits,
warnings about install-time scripts and non-registry dependencies; `tests/hostile_templates.rs`), what it cannot do, and what a
host must show the user. `docs/authoring-templates.md` is the guide to writing one. A separate checking engine is planned and is not part of this crate; `docs/integration-notes.md` has notes on how it, the template engine and a host such as the Helm TUI could fit together (nothing built).

## Not built yet

- the pack registry (generator folders are made by `tools/make_generator.py` in the templates repo, and `pack_dir` / `cargo run --example pack` turns them into packs)
- the `.mentenaz-template.json` marker file (optional now: adding to a solution detects it from its files)
- adding extensions, libraries and Adaptive Card extensions to a solution (`docs/add-to-solution.md`). JSON with comments and trailing commas is edited as text (`src/jsonc.rs`), so they stay. `NodeCheck` reports whether the installed Node fits a template or solution. `cargo run --example host_flow` shows the whole host flow.
- the NuGet source (`dotnet new` templates)
- the panel UI (in the fork, not this crate)

## Repos

| Repo | Holds | State |
|---|---|---|
| `E:\mentenaz\Forge.Scaffold.SDK` (this crate) | the engine | built, tested, not yet published |
| `E:\mentenaz\Forge.Scaffold.Templates` | raw generator output, JSON Schemas (`manifest.v1.json`, `pack.v1.json`), a sample pack | early: git repo exists (nothing committed yet), no packs built yet |
| `E:\vite` | upstream Vite monorepo, reference data for the `vite-react` generator | read-only reference |

## Generators built so far

SPFx web part generators for 1.20.0, 1.21.1, 1.22.2, 1.23.0 and 1.23.2 live in the templates repo under
`templates/spfxv.<version>/`. Each was scaffolded with two web parts, installed and built (see
`docs/spfx-generator-findings.md`). `cargo test --test spfx_generators` scaffolds all of them, and
`cargo run --example scaffold` does it by hand. `pack_dir` turns any of them into one pack file, and a pack scaffolds byte for byte like its folder (tested for all five). `build_release` (`cargo run --example release`) builds the archives and `index.json`; the test installs all five through the real `TemplateStore`. `build_release_from` also takes upstream templates (`ReleaseInput::Upstream`) and converts them to a generator folder at release time, so clients only ever see one format. Hosting and upload: `docs/publishing.md`. The official `SharePoint/spfx` templates are read directly by `upstream` (`cargo run --example upstream`); see `docs/upstream-spfx-findings.md`.

## Design rules already enforced in code

- Output always goes into a brand-new folder. If it exists, the error is
  "The current folder already exists, please choose a different name" and the
  existing folder is never touched.
- If an IO error happens halfway, the folder that was just created is removed.
- Unknown `{__tokens__}` in a template are an error, not silent text.
- Manifests reject unknown fields, so a typo in a key is caught early.
- `when` conditions and `packageManagers` are cross-checked against the prompts at load time (unknown keys, values outside a choice's options, a default that is not in `tested`).
- Template and output paths cannot be absolute, contain `..`, or use characters Windows rejects.

## Templates folder

```text
<root>/index.json        cached remote index (ETag + last check)
<root>/installed.json    the folders this crate downloaded
<root>/spfxv.1.23.2/     manifest.json + template folders
<root>/my-own/           your own folder: never touched, wins over downloads
```

Root: the path the host passes in, else `FORGE_TEMPLATES`, else `<data dir>/mentenaz/templates`.
Downloads are verified (`sha256`), extracted into a scratch folder and swapped in atomically.
A folder that is not listed in `installed.json` is never overwritten.

## Template provenance

Where every file of the templates repo comes from is written down in its `README.md` (the captures, the generators made from
them, the artwork, the official templates, and what was and was not checked). Templates must come from a fresh
`@microsoft/generator-sharepoint` run with dummy names and no work code.

Captured so far: SPFx `1.20.0`, `1.21.1`, `1.22.2`, `1.23.0`, `1.23.2`, each a
`yo @microsoft/generator-sharepoint` output under
`E:\mentenaz\Forge.Scaffold.Templates\generators\spfx-webpart\<version>`. Before any of
it can be packed:

- `node_modules/`, `package-lock.json`, `temp/`, `lib/` and `lib-commonjs/` must
  be stripped or excluded — they are build output, not template.
- `.yo-rc.json` carries the dummy answers used at generation time.
- The output is **not** pristine: `1.20.0`/`1.21.1` include a `fast-serve/`
  folder and all versions carry a custom `Types/` folder. Diff against a clean
  `yo` run and decide what to keep.
- The Microsoft licence question for the `yo`-derived output is open (see `LICENSING.md` in the templates repo), and the 1.23.2 capture was diffed against a fresh `yo` run (21 Nov 2026): it is **not** unmodified `yo` output, see `docs/spfx-generator-findings.md` (the other four versions have not been diffed).

## Known data drift

The `spfxVersion` to `version` rename is fixed in `Manifest.v1.json`. The old hand-written sample pack, whose icon was a
truncated placeholder, has been replaced by a real one (`spfx-webpart-1.23.2-r1.pack.json` in the templates repo, made with
`cargo run --example pack -- <generator folder> <revision> <output>`), which loads.

## Build and test (PowerShell)

```powershell
cd E:\mentenaz\Forge.Scaffold.SDK
cargo test
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this crate is dual licensed as above,
without any additional terms or conditions.

This covers the engine only. The SPFx templates (sibling repo) are derived from
Microsoft's `@microsoft/generator-sharepoint` output and are not covered by this
licence: see `LICENSING.md` in that repo.
