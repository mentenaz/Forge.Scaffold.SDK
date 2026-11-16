# SPFx: what `yo` changes when a component is added to an existing solution

Measured, not assumed. Every result below comes from running the real generator and diffing the solution
before and after. It is the specification for an "add to existing solution" mode in the engine.

## Method

- Generators: `@microsoft/generator-sharepoint` 1.20.0, 1.21.1, 1.22.2, 1.23.0, 1.23.2, each run under the Node
  version it is installed for (nvm): 18.20.8, 22.21.1, 22.22.0, 22.22.3, 22.23.0.
- Flags: `--skip-cache --skip-install --package-manager npm`, so no `node_modules` or lockfile noise.
- Per cell: create a solution with one component (`AlphaWebPart`), snapshot it (`pre`), run the generator again
  inside it to add a second component (`BetaWebPart`/`BetaExt`/...), snapshot it (`post`), then
  `git diff --no-index pre post`. Component names and GUIDs are normalised before comparing.
- Compared per cell: modified files, number and names of new files, and a hash of the `config/config.json` edit.

## 1. Adding a web part

24 cells (5 versions, default and `--use-gulp` builds where the flag exists, 3 frameworks). All passed, and all
agree.

| | |
|---|---|
| Existing files modified | `.yo-rc.json`, `config/config.json` — nothing else |
| `config/config.json` edit | identical in every cell (same hash) |
| New files, framework `none` | 9 |
| New files, framework `minimal` | 7 |
| New files, framework `react` | 11 |

The new files are the web part folder under `src/webparts/<camelName>/` plus two Teams icons named
`<new guid>_color.png` / `<new guid>_outline.png` under `teams/`.

The `config/config.json` edit has two parts:

1. `bundles.<kebab-name>-web-part` = `{ "components": [ { "entrypoint": "./lib/webparts/<camel>/<Pascal>WebPart.js",
   "manifest": "./src/webparts/<camel>/<Pascal>WebPart.manifest.json" } ] }`
2. `localizedResources.<Pascal>WebPartStrings` = `lib/webparts/<camel>/loc/{locale}.js`

`.yo-rc.json` only records the generator's last answers (`componentName`, `componentDescription`,
`isCreatingSolution: false`). The build does not read them.

Adding a third web part changes the same two files and adds the same file set, so the edit grows linearly.

## 2. Build system is a per-solution choice, not a per-version one

- 1.20.0 and 1.21.1: gulp only. `.yo-rc.json` has no `useGulp` field.
- 1.22.2, 1.23.0, 1.23.2: heft by default; `--use-gulp` produces a gulp project (`gulpfile.js`, `.eslintrc.js`)
  and records `useGulp: true`.
- A clean `yo` run never contains `fast-serve/`. That folder is an add-on for gulp solutions.
- A clean `yo` run never contains a `Types/` folder.

`.yo-rc.json` holds what detection needs: `version`, `useGulp`, `template`, `componentType`, `solutionName`,
`packageManager`, `libraryId`.

## 3. Other component types

Identical results on all five versions for each type.

| Type | Fresh solution (files) | Added to existing: new files | Existing files modified |
|---|---|---|---|
| ApplicationCustomizer | 23 | 6 | `config.json`, `package-solution.json`, `serve.json`, `package.json` |
| FieldCustomizer | 23 | 6 | same |
| ListViewCommandSet | 23 | 6 | same |
| SearchQueryModifier | 23 | 6 | same |
| FormCustomizer | 22 | 5 | same, without `package-solution.json` |
| library | 21 (1.20, 1.21) / 22 | 4 / 5 | `config.json`, `package.json` |
| Adaptive Card: Generic / Search / DataVisualization | 26 / 28 / 25 | 9 / 11 / 8 | `config.json`, `package.json` |

Edits seen in 1.23.2:

- Customizers: `package.json` gains `@microsoft/sp-application-base`, `@microsoft/sp-dialog`, `@microsoft/decorators`;
  `package-solution.json` gains `assets.elementManifests`; `serve.json` gains `serveConfigurations` keyed by a GUID.
- Library: `package.json` `"private"` becomes `false`.
- Adaptive Card: `package.json` gains `@microsoft/sp-adaptive-card-extension-base`.

## Caveats

- For extensions, library and Adaptive Card the add was run with `--framework none` inside a react solution. The
  generator then also switched `eslint.config.js` from the `react` profile to `default` and rewrote the saved
  template in `.yo-rc.json`. That edit is very likely caused by the mismatched flag, not by the component. It was
  not retested with a matching framework. An engine add must not change the solution's template or lint profile.
- Extensions, library and Adaptive Card were not tested with `react`/`minimal` frameworks.
- Hand-edited solutions were not tested.
- Not covered: `--is-domain-isolated`, `--skip-feature-deployment`, `--plusbeta`, package managers other than npm.
