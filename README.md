# Forge.Scaffold

Rust-native scaffolding engine for SPFx templates (Yeoman-style, no Node).

## Status

All pure logic. The crate opens no connections itself, the host supplies HTTP through the `Fetch` trait. 74 tests.

| Module | What it does |
|---|---|
| `manifest` | Generator manifest as data: prompts (`text`, `bool`, `list`, `choice`, `sensitive`, name `rule`), `include`, `repeat`, `parts` (ordered fragments), `merge` (`value` or `valueFile`), `secrets`, `post` steps, `when` (`equals`, `in`, `all`), `packageManagers` |
| `plan` | `plan()` renders everything in memory and returns files, warnings (nested solution, long path, untested package manager) and post-step *descriptors*. `apply()` is the only writer |
| `fetch` | The `Fetch` trait (HTTP GET with ETag) the host implements |
| `index` | The remote `index.json` format (`sha256`, `revision`, https-only URLs) plus the local cache and `installed.json` |
| `archive` | Safe `.tar.gz` extraction: files and folders only, path checks, caps on file count and size |
| `store` | The templates folder: version list (newest first, works offline), `resolve`, `check_updates` (24 h interval, ETag, force), `update`, `auto_update` with events, lock file, atomic swap |
| `secrets` | Generated alphanumeric secrets (32 chars, always upper + lower + digit) |
| `tokens` | `{__token__}` rendering on paths and content, derived forms (`Pascal`, `Camel`, `Kebab`), stable per-scope GUID tokens |
| `stage` | In-memory staged filesystem, case-insensitive duplicate and file/folder conflict detection, "new folder only" writer with rollback |
| `validate` | Name validation for the solution and web parts, returned as data for the panel |

Not built yet: the `.mentenaz-template.json` marker, the NuGet source.

The crate never runs commands. `install` and `composeUp` come back as descriptors for the host's script runner.

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

## Build and test (PowerShell)

```powershell
cd H:\path\to\Forge.Scaffold
cargo test
```

## License

Not chosen yet.
