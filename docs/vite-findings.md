# create-vite templates: what was checked

The Vite generators are made from the templates of `create-vite` (`packages/create-vite` of `vitejs/vite`, MIT, version
9.2.1 when this was written; the fork is `mentenaz/vite`). The engine reads them directly (`src/create_vite.rs`) and the
release builder converts them at release time (`ReleaseInput::CreateVite`, `release --create-vite <repo> <template>`), the
same way as the official SPFx templates. Nothing derived is committed.

## What create-vite does to a template, and what the engine does

| create-vite (`src/index.ts`) | engine |
|---|---|
| copies the `template-*` folder | `include` of the translated folder |
| `_gitignore` is written as `.gitignore`, `_oxlintrc.json` as `.oxlintrc.json` | the manifest's `rename`, which any generator can use |
| `package.json`: `pkg.name = packageName`, written with 2 spaces and a final newline | the `name` becomes `{__projectName__}` (key order kept) |
| `index.html`: the text in `<title>...</title>` becomes the package name (one line only) | the same text becomes the token |
| the package name is validated, or fixed up with `toValidPackageName` | the `package-name` rule, and `suggest_package_name` for pre-filling |
| installs and starts the dev server | the `install` step comes back as a descriptor for the host; starting is not done |
| ESLint and React Compiler options patch in files | not done |
| `custom-*` templates (Nuxt, React Router, TanStack, ...) call other CLIs | not done: not data |

The reader refuses what it does not expect instead of guessing: a file named with a leading `_` that has no rename, a template
whose `package.json` has no `name`, and file text that already contains `{__`.

## Checked (20 November 2026, Windows)

- Unit tests for the translation, scaffolding, the name rule, the refusals, and the release (notice, licence, archive
  name `vite-react-ts.9.2.1`, deterministic bytes, refusal without the LICENSE file).
- `tests/create_vite_templates.rs` against the real fork: **all 16 templates** (`lit`, `preact`, `qwik`, `react`, `solid`,
  `svelte`, `vanilla`, `vue`, each in JavaScript and TypeScript) scaffold, and each project is the template folder with exactly the
  three changes above, file for file, byte for byte. Skipped when the Vite repo is not there (`../vite` or `FORGE_UPSTREAM_VITE`).
- By hand: `react-ts` scaffolded with `cargo run --example create_vite`, compared with a copy of the template made with plain
  shell commands (identical), then `npm install` (27 packages) and `npm run build` (`tsc -b && vite build`, Vite 8.3.8) on
  Node 22.23.0: a clean build. The same build under Node 18 fails inside Rolldown (`styleText` is not in `node:util`), which
  is why the manifest carries create-vite's `engines.node` (`^20.19.0 || >=22.12.0`) as `requires.node`.

## Not checked

- Only `react-ts` was installed and built; the other 15 were only compared with their template folders.
- The output was compared with the template, not with what `npm create vite` writes for the same inputs (Node 22 and the network
  were available, but `create-vite` itself was not run). The translation follows `src/index.ts` line by line.
- The templates carry Vite's and the frameworks' logos (SVG files). They are part of the MIT-licensed templates, but the
  names and logos are trademarks of their owners; decide whether to replace them with an overlay before a public release.
- The publish workflow does not release these yet (it needs a checkout of the Vite fork and a list of templates).
