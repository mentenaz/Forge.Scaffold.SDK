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

## Not covered

- `heft package-solution` / `.sppkg`, `heft start`, and running a web part in a workbench.
- The `install` post step (the engine only returns a descriptor).
- A web part name that already ends in `WebPart`.
- Frameworks other than React, and `--use-gulp` on 1.22.2 and later.
- A clean `npm install` under Node 18 for 1.20.0.
- Provenance: the captures still need the diff against a clean `yo` run and the Microsoft licence check.

## Regression test

`tests/spfx_generators.rs` scaffolds every `spfxv.*` folder and checks: no unresolved `{__` token, both web parts exist,
`config.json` lists both bundles and strings, four Teams icons, and `.yo-rc.json` carries the new solution name. It
skips when the templates repo is not next to this one (override with `FORGE_TEMPLATES_REPO`).
