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

## Forge artwork in the Vite template

The release builder's overlay (`layers/overlay.json` in the templates repo) also applies to a create-vite template
(`ViteSource::apply_overlay`). A rule replaces the files whose path ends with the rule's path; a key may start with
`<template>:` (`react-ts:src/App.tsx`) to apply to that template only, which matters because `src/App.tsx` means something
else in Solid or Preact; and a `"remove"` list leaves files out. The text of a replacement may use tokens, such as the
project name. The SPFx side ignores `remove` and the prefixed keys.

For `vite-react-ts` the overlay (8 files changed in all):

- replaces `public/favicon.svg` with the Forge vector mark and `src/assets/hero.png` with the Forge welcome image;
- replaces `src/App.tsx`, `src/App.css` and `README.md` with a small Forge page (logo, project name, a counter) and a plain
  README (`layers/vite/react-ts/`);
- removes `src/assets/vite.svg`, `src/assets/react.svg` and `public/icons.svg`, so the project has no Vite or React logo and
  no links to Vite's GitHub, Discord, X or Bluesky.

What is left that names Vite: the `vite` and `@vitejs/plugin-react` dependencies, the scripts, `vite.config.ts`, the
`tsconfig` files that mention it, and one factual line in the README ("built with React, TypeScript and Vite"). The
`NOTICE.txt` credits Vite and says how many files the overlay replaced or left out, which are not covered by Vite's licence.

Checked: the 25 SPFx archives are byte for byte unchanged (the new rules match nothing in them); a project made from the
archive has 15 files, installs, builds and lints with exit code 0 on Node 22.23.0, and the page renders in Chrome (headless,
dark theme) as the Forge logo, the project name, the welcome text and the counter.

The mark is detailed line art with a wordmark; the shape renders correctly, but the detail turns into a blur at 16 to 32 pixels, so
a simpler mark (the flame and anvil alone) may suit a favicon better. The light and dark colours of the favicon rely on a
`prefers-color-scheme` rule inside the SVG that was not tested in a browser.

## Not checked

- Only `react-ts` was installed and built; the other 15 were only compared with their template folders.
- The output was compared with the template, not with what `npm create vite` writes for the same inputs (Node 22 and the network
  were available, but `create-vite` itself was not run). The translation follows `src/index.ts` line by line.
- The light theme of the page was not looked at, and the favicon was not seen in a browser tab (see the notes above on its detail).
- Only `react-ts` has an overlay for its page. Releasing another template (`vue-ts`, `svelte-ts`, ...) would still carry
  its own Vite and framework logos and demo page until a `<template>:` rule is added for it.
- The publish workflow does not release these yet (it needs a checkout of the Vite fork and a list of templates).
