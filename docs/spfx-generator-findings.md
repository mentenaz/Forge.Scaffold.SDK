# SPFx generators: scaffolding from the captured templates

The engine was run end to end on the raw `yo @microsoft/generator-sharepoint` captures (1.20.0, 1.21.1, 1.22.2,
1.23.0, 1.23.2): capture → generator folder → `plan()` / `apply()` → install → build. No engine change was needed.

## From capture to generator folder

`tools/make_generator.py <version>...` in `Forge.Scaffold.Templates` turns `generators/spfx-webpart/<version>/` into
`templates/spfxv.<version>/`. It reads git-tracked files only, so `node_modules`, `lib`, `temp` and similar never get in.

```text
templates/spfxv.<version>/
  manifest.json     prompts, GUIDs, repeat, two merges into config/config.json, install post step
  solution/         copied once: package.json, config/, .yo-rc.json, tsconfig.json, ...
  webpart/          expanded once per web part name (src/webparts/<camel>/, its Types, its Teams icons)
```

- Prompts: `solutionName`, `solutionDescription`, `webparts` (list), `install` (bool).
- GUIDs: `solutionId` (also `libraryId`), `featureId`, and one `webpartId` per web part (manifest id and Teams icon names).
- `config/config.json` ships with empty `bundles` and `localizedResources`. Two merges per web part fill them:
  `bundles.<kebab>-web-part` and `localizedResources.<Pascal>WebPartStrings`.
- `package-lock.json` and generated `*.scss.ts` are left out.

Try it: `cargo run --example scaffold -- <generator folder> <location> <solutionName> HelloWorld,OtherPart`.

## Result per version

Each solution was scaffolded with two web parts (`HelloWorld`, `OtherPart`), installed with npm, and built.

| Version | Build tool | Files written | Build | Node |
|---|---|---|---|---|
| 1.23.2 | heft | 43 | passed | 22.23.0 |
| 1.23.0 | heft | 41 | passed | 22.23.0 |
| 1.22.2 | heft | 42 | passed | 22.23.0 |
| 1.21.1 | gulp | 46 | passed | 22.23.0 |
| 1.20.0 | gulp | 41 | passed | 18.20.8 |

No errors or warnings in the build output. Install takes 5 to 16 minutes and 1300 to 2800 packages.
1.20.0 fails under Node 22 (`gulp build` refuses to start), which is the Node range of that SPFx line, not a template
problem. Its `node_modules` came from an install under Node 22 (an `EBADENGINE` warning, completed); a clean install
under Node 18 was not run.

## What differs between the captures

- **Build system.** 1.20.0 and 1.21.1 are gulp (`gulpfile.js`, `.eslintrc.js`, `fast-serve/`); 1.22.2 and later are
  heft. The 1.22.2 capture also has a `.eslintrc.js`.
- **Name of the capture solution.** 1.23.x used `1-23-2` style names. 1.20.0 to 1.22.2 used folder name `template` and
  display name `Template`, so the script replaces both, as whole words and never JSON keys.
- **Comments in JSON.** The 1.22.2 web part manifest has comments, so the script reads GUIDs by pattern.
- **Types.** 1.21.1 and later have the custom `Types/` folder with a barrel (`src/Types/index.ts`). 1.20.0 has none
  and keeps `components/ITemplateProps.ts`. Only 1.23.2 has `DemoComponent.tsx`.

## Decisions made in the generator

- **Per-web-part `Types` imports.** A shared barrel would be written once per web part and collide. The barrel is
  dropped and imports point at each web part's own files, e.g. `Types/IHelloWorld/IHelloWorldProps`.
- **Customizations kept.** `DemoComponent.tsx` and the `react-hook-form`, `zod` and `@hookform/resolvers` dependencies
  stay in the 1.23.2 template.
- **`fast-serve/` kept** in 1.20.0 and 1.21.1, as captured. A clean `yo` run does not contain it
  (`spfx-add-findings.md`), so it is a question for the provenance pass.
- **`tsconfig.json`.** `baseUrl` sits inside `paths` in the capture, probably a typo. It compiles anyway and was left as is.

## Packaging (1.23.2)

On a one-web-part solution, `heft build --production` and `heft package-solution --production` both pass and produce
`sharepoint/solution/<name>.sppkg` (about 44 KB). The package log lists the web part manifest, the bundle, the strings,
both images and both Teams icons. Only 1.23.2 was packaged. The package was not deployed to a tenant.

## Web part names ending in `WebPart`

The templates always append `WebPart` (`HelloWorld` gives `HelloWorldWebPart.ts`). A name that already ends in it would
give `HelloWorldWebPartWebPart`, so `validate_webpart_names` now rejects it ("Leave out 'WebPart' at the end: it is added
for you"). A name that only contains the word, like `WebPartList`, is fine. This was a choice made for the templates, not
something measured from `yo`.

## Packs

`pack_dir` (`cargo run --example pack -- <generator folder> <revision> <output>`) packs a generator folder. For all five
versions, scaffolding from the pack gives byte-identical output to scaffolding from the folder (same GUID sequence,
newline normalization off). The 1.23.2 pack is about 80 KB.

## Not covered

- Packaging for versions other than 1.23.2, deploying an `.sppkg`, `heft start`, and running a web part in a workbench.
- The `install` post step (the engine only returns a descriptor).
- Frameworks other than React, and `--use-gulp` on 1.22.2 and later.
- A clean `npm install` under Node 18 for 1.20.0.
- Provenance: the captures still need the diff against a clean `yo` run and the Microsoft licence check.

## Regression test

`tests/spfx_generators.rs` has two tests. The first scaffolds every `spfxv.*` folder and checks: no unresolved `{__` token, both web parts exist,
`config.json` lists both bundles and strings, four Teams icons, and `.yo-rc.json` carries the new solution name. It
skips when the templates repo is not next to this one (override with `FORGE_TEMPLATES_REPO`).

The second scaffolds each folder and a pack of it, and compares every file.

## The 1.23.2 capture compared with a fresh `yo` run (21 November 2026)

`@microsoft/generator-sharepoint@1.23.2` was installed in a scratch folder and run on Node 22.23.0 with the answers recorded in
the capture's `.yo-rc.json` (`--solution-name 1-23-2 --component-type webpart --component-name template --framework react
--environment spo --package-manager npm --skip-feature-deployment --skip-install`), and the result compared with the git-tracked
files of `generators/spfx-webpart/1.23.2` (line endings ignored). Of the 32 tracked files, 28 are produced by `yo`; 18 of those
are identical. The rest differ, and the capture is **not unmodified `yo` output**:

- **Extra files that `yo` does not create:** `src/Types/` (three files), `src/webparts/template/components/DemoComponent.tsx`
  (and `yo` creates `components/ITemplateProps.ts`, which the capture does not have).
- **Extra dependencies:** `react-hook-form`, `@hookform/resolvers` and `zod` (plus `eslint-config-spfx` and a few others), and a
  `compile` script (`heft build`). `tsconfig.json` has a `paths` block for `Types`.
- **The web part rewritten:** `Template.tsx` is a function component (`yo` writes a class), the web part imports `Types`, and the
  files are formatted with double quotes (`yo` uses single quotes). The web part manifest has its comments removed and a
  different layout.
- **Only differences that are expected:** the generated ids (solution, library, feature, web part), `.yo-rc.json` key order and
  the Teams icon file names, which follow the web part id.

What this means: the `spfxv.1.23.2` generator, made from this capture, ships `zod`, `react-hook-form` and a demo component that
the official generator does not. The README and licence statements that call these generators "derived from Microsoft's
generator output" are only partly accurate for 1.23.2: they are derived from it **and edited**. The owner then had the three packages and `DemoComponent.tsx` removed from 1.23.2 (the rest of the list above was left as it is); a
scaffolded solution was installed and built on Node 22.23.0 afterwards and produced a `.sppkg`.

### The other four versions (22 November 2026)

The same comparison for 1.20.0, 1.21.1, 1.22.2 and 1.23.0 (`yo` run with each capture's own answers; ids ignored). No version has
the form packages or the demo component. All four captures are edited in the same ways, which are not what `yo` writes:

| | 1.20.0 | 1.21.1 | 1.22.2 | 1.23.0 |
|---|---|---|---|---|
| Files identical to `yo` | 23 of 28 | 21 of 28 | 21 of 28 | 21 of 28 |
| `spfx-fast-serve` added (`fast-serve/`, `spfx-fast-serve-helpers`, `serve` script, `gulpfile.js` hook) | yes | yes | no | no |
| `src/Types/` folder, and `src/custom.d.ts` | no | yes | yes (and `custom.d.ts`) | `Types/` only |
| `tsconfig.json` `paths` block for `Types` | no | yes | yes | yes |
| `compile` script (`heft build`) | no (gulp) | no (gulp) | yes | yes |
| Other `package.json` differences | none | `build`, `test` scripts changed, `production` script added | none | none |
| `components/ITemplateProps.ts` moved into `Types/` | no | yes | yes | yes |
| Web part and component rewritten (double quotes, trailing commas, comments removed in the manifest) | yes | yes | yes | yes |

`spfx-fast-serve` is a community add-on, not part of Microsoft's generator. So every generator is "derived from `yo` output and
then edited", not plain `yo` output: the same statement as for 1.23.2. Nothing else was changed.
