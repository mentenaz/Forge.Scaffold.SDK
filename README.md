# Forge.Scaffold

Rust-native scaffolding engine (Yeoman-style, no Node). Generators are data: a manifest plus template files, read from a folder or from a pack compiled into the app.

## Status

All pure logic. The crate opens no connections itself, the host supplies HTTP through the `Fetch` trait. 116 tests, passing on Windows.

| Module | What it does |
|---|---|
| `manifest` | Generator manifest as data: prompts (`text`, `bool`, `list`, `choice`, `sensitive`, name `rule`), `include`, `repeat`, `parts` (ordered fragments), `merge` (`value` or `valueFile`), `secrets`, `post` steps, `when` (`equals`, `in`, `all`), `packageManagers` |
| `source` | The `TemplateSource` trait ("list the files below a folder", "read a file") that `plan()` reads templates through, plus `DirSource` for a real folder (symlinks refused) |
| `pack` | `Pack::from_json`: one generator version in a single file (manifest + every template file, text or base64). A file's content is inline or stored once in `blobs` under its SHA-256 and referenced by that hash, so a file used in many variants costs its size once. Loading checks the schema version, safe and unique paths, every hash, size limits (`PackLimits`), and that the manifest only refers to folders and files that exist. A `Pack` is a `TemplateSource`. Format: `pack.v1.json` |
| `packer` | `pack_dir`: a generator folder in, one `pack.v1.json` out. Text as `utf8`, everything else as `base64`, repeated content stored once in `blobs`, deterministic output, optional newline normalization, a configurable exclude list. The result is loaded back with `Pack::from_json` before it is returned |
| `plan` | `plan()` renders everything in memory and returns files, warnings (nested solution, long path, untested package manager) and post-step *descriptors*. `apply()` is the only writer |
| `fetch` | The `Fetch` trait (HTTP GET with ETag) the host implements |
| `index` | The remote `index.json` format (`sha256`, `revision`, https-only URLs) plus the local cache and `installed.json` |
| `archive` | Safe `.tar.gz` extraction: files and folders only, path checks, caps on file count and size |
| `store` | The templates folder: version list (newest first, works offline), `resolve`, `check_updates` (24 h interval, ETag, force), `update`, `auto_update` with events, lock file, atomic swap |
| `secrets` | Generated alphanumeric secrets (32 chars, always upper + lower + digit) |
| `tokens` | `{__token__}` rendering on paths and content, derived forms (`Pascal`, `Camel`, `Kebab`), stable per-scope GUID tokens |
| `stage` | In-memory staged filesystem, case-insensitive duplicate and file/folder conflict detection, "new folder only" writer with rollback |
| `validate` | Name validation for the solution and web parts, returned as data for the panel |

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
let pack = Pack::from_json(include_str!("spfx-1.23.2-r1.pack.json"))?;
let planned = plan(pack.manifest(), &pack, &answers, PlanOptions::new(location))?;
apply(&planned, |_| {})?;
```

`manifest.json` may start with a `"$schema"` line (ignored by the engine) so editors can validate it against `manifest.v1.json`.

The crate never runs commands. `install` and `composeUp` come back as descriptors for the host's script runner.

## Not built yet

- `when` on prompts (a prompt that only shows conditionally)
- the `package-name` name rule
- the pack registry (generator folders are made by `tools/make_generator.py` in the templates repo, and `pack_dir` / `cargo run --example pack` turns them into packs)
- the `.mentenaz-template.json` marker file
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
`cargo run --example scaffold` does it by hand. `pack_dir` turns any of them into one pack file, and a pack scaffolds byte for byte like its folder (tested for all five).

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

## Template provenance (fill in before publishing)

Templates must come from a fresh `@microsoft/generator-sharepoint` run with
dummy names and no work code. Record the exact version here and check that
package's license before publishing.

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
- The Microsoft licence check and a provenance README are still to do (see `LICENSING.md` in the templates repo).

## Known data drift

The `spfxVersion` to `version` rename is fixed in `Manifest.v1.json` and the
sample pack. The sample pack (`E:\mentenaz\Forge.Scaffold.Templates\spfx-1.23.2-r1.pack.json`) still does
not load: `base/assets/icon.png` holds a truncated placeholder
(`iVBORw0KGgo…`), which is not valid base64. Replace it with real data or build
the pack with `cargo run --example pack -- <generator folder> <revision> <output>`.

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
