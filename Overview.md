## The idea

A Rust-native, Yeoman-style scaffolding engine for your Zed/GPUI fork, with no Node. It is also a portfolio piece. Three parts make it work:

| Part                                | Where                                                                                               | Status                        |
| ----------------------------------- | --------------------------------------------------------------------------------------------------- | ----------------------------- |
| **Engine** (`forge-scaffold` crate) | This repo, used as a git dependency in the fork                                                     | Built, 116 tests on Windows, clippy clean |
| **Templates**                       | Separate repo (`E:\mentenaz\Forge.Scaffold.Templates`), published as release archives plus `index.json`      | Raw generator output only — no packs, no index, nothing committed yet |
| **Panel**                           | Your fork: generator list on the left, form in a workspace tab                                      | Yours to build                |

## Generators

- **SPFx (flagship):**
  - Basic web parts only. Raw `yo @microsoft/generator-sharepoint` output has been captured for `1.20.0`, `1.21.1`,
    `1.22.2`, `1.23.0` and `1.23.2` under `generators/spfx-webpart/`. On 1.23.2 the main component is already a
    function component (`export const Template = (props) => …`).
  - Not pristine: `1.20.0`/`1.21.1` carry a `fast-serve/` folder and every version has a custom `Types/` folder, so a
    provenance pass against a clean `yo` run is still owed.
  - The panel flow is: pick a version, see current dependencies, search npm packages compatible with that version, toggle `npm i`, pick a location, scaffold.
- **Full-stack scaffold** (from your old PowerShell wizard):
  - Vite React TS client, Node/Express/Sequelize server, and a database choice (Postgres, SQLite, MySQL or MSSQL).
  - Docker, hybrid or native presets, a `.env` with generated secrets, and a generated Forge flow file.
- **Later, optional:** a NuGet source that lists `dotnet new` templates dynamically and runs the ones the engine can support.

## What the engine covers today

- **Manifests:** declarative JSON with no code, covering prompts, `repeat` per list item, `include`, `parts`, JSON `merge`, generated secrets, `when` conditions and post steps. Typos in a manifest are rejected.
- **Rendering:** `{__token__}` replacement in paths and contents, with Pascal, Camel and Kebab forms and stable GUIDs. An unknown token is an error.
- **Planning:** `plan()` builds everything in memory. It returns the files, warnings (nested solution, long Windows path, untested package manager) and post-step descriptors. It writes nothing.
- **Writing:** `apply()` writes into a brand-new `[location]/[name]` folder and refuses if it exists. A failed write removes the folder it created.
- **Validation:** strict solution and web part names, because bad names have broken SPFx manifests before.
- **Template management:**
  - Versions are listed newest first and work offline.
  - Archives are verified by `sha256` and extracted safely.
  - Updates check on a 24 h interval with ETag, with a manual force check and an auto-update toggle.
  - Folders you made yourself are never overwritten.
- **Packs:** `Pack::from_json` holds one generator version in a single `pack.v1.json` — manifest plus every template file, content deduped into `blobs` by SHA-256. It validates paths, hashes and size caps, and is itself a `TemplateSource`, so the same generator can be read from a folder or from bytes compiled into the app.

## What it deliberately does not do

- **Run commands.** `npm i` and `docker compose up` come back as descriptors, and your ScriptRunner executes them.
- **Make network calls itself.** The panel supplies `Fetch` using the fork's HTTP client.
- **Draw any UI.**
- **Include work code.** No employer code, names, tenant URLs or libraries, and no UI or service layers from work. Templates start from a clean history.
- **Cover everything Yeoman does.** There are no JS generators or plugins, and no loops beyond `repeat`.
- **Run the .NET template language** (no `repeat` or merge there). That is the optional NuGet source for later.

## Still open

1. **Version field — done.** `Manifest.version` replaced `spfxVersion` in the engine, `Manifest.v1.json` and the sample pack.
   The sample pack still fails to load because `base/assets/icon.png` is a truncated placeholder, not valid base64.
2. **Marker file:** `.mentenaz-template.json` is not built, and the filename is unresolved (leading dot or not).
3. **npm compatibility filter:** where "works with this SPFx version" data comes from (the template's `package.json`, a pinned list, or peer ranges).
4. **Templates repo:** exists at `E:\mentenaz\Forge.Scaffold.Templates` with raw `yo` output for SPFx 1.20.0 → 1.23.2, a
   generator folder per version (`templates/spfxv.<version>/`, all five scaffold and build) and the two JSON Schemas (plus
   empty `fullstack/`, `vite-react/`, `backend-api/` folders). Still to do: build the release `.pack.json` files, `index.json`, the release archives, the Microsoft licence check and a provenance README.
   `.github/workflows` is empty.
5. **Crate licence:** the crate needs one before it goes public. I suggested `MIT OR Apache-2.0`.
6. **Windows test: done.** `cargo test` passes on this machine (116 tests). The rename and long-path behaviour are covered.
7. **Flow file details:** whether a one-shot action type exists, and whether a deterministic `flow-<projectName>` id is fine.
8. **Vite templates:** `E:\vite` is the upstream monorepo kept as reference data for the `vite-react` generator. Confirm
   which packages (`create-vite` templates) it should be built from before writing that generator.

## Suggested order

1. Replace the sample pack's placeholder `icon.png` (or rebuild the pack), decide the marker name, then build the marker.
2. Packer: done (`pack_dir`, `cargo run --example pack`). Produce the first release packs from `templates/spfxv.<version>`.
3. Add `index.json`, the release archives + workflow, and the provenance README with the Microsoft licence check.
4. Choose the crate licence and make the crate public, then wire it into the fork as a git dependency.
5. Build the panel against it.
6. Add the `fullstack` and `vite-react` generator folders and, last, the NuGet source.
