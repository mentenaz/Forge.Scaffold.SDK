# Scaffolding from the official SharePoint/spfx templates

Microsoft now ships an open-source SPFx CLI, `SharePoint/spfx` (the replacement for the Yeoman generator). Its
templates are data in the same spirit as ours, so the engine reads them directly instead of going through captured
`yo` output. This records what was checked. The fork is `mentenaz/spfx`.

## The upstream repo

- Root `LICENSE`: "The projects in this monorepo are licensed under the MIT license", copyright Microsoft Corporation.
  The CONTRIBUTING terms and the headers of individual templates were not read.
- 20 templates under `templates/`, all `spfxVersion` 1.24.1 on `main`: web parts (`minimal`, `noframework`, `react`),
  extensions (application customizer, field customizer x3, form customizer x2, list view command set, search query
  modifier), five Adaptive Card extensions, `library`, and three `copilot-component-*`.
- Upstream selects an SPFx version by branch (`--spfx-version 1.22` means a `version/1.22` branch). Only `version/latest`
  and `version/next` exist upstream today, so older SPFx versions are not available from it.
- A template is a folder with `template.json` (`name`, `category`, `description`, `version`, `spfxVersion`) and the files
  of a whole solution. Contents use EJS tags, paths use placeholders:
  `<%= componentName.pascal %>`, `src/webparts/{componentName.camel}WebPart/`, `teams/{componentId}_color.png`.
- Across the template files that use tags, every tag is a plain `<%= name %>` or `<%= name.casing %>`. No logic.
- Upstream also has merge helpers for `config.json`, `package.json`, `package-solution.json` and `serve.json`, which is how
  it adds a component to an existing solution (the same four files `spfx-add-findings.md` measured).

## How the engine reads them (`src/upstream.rs`)

`UpstreamSource::load` reads a template folder through any `TemplateSource` and translates it once into this crate's
`{__token__}` form. After that it is the ordinary pipeline: `template.manifest()` gives the manifest, `UpstreamInputs`
gives the answers, then `plan()` and `apply()`.

- Casings `pascal`, `camel`, `hyphen` (our `Kebab`) and `allCaps` (new: `nameAllCaps` in `TokenMap::insert_name`).
- Variables: `componentName`, `libraryName`, `solutionName` (also `solution_name`), `componentAlias`,
  `componentDescription`, the GUIDs `componentId`, `featureId`, `solutionId`; `spfxVersion` and
  `spfxVersionForBadgeUrl` come from `template.json`.
- The `_phase:*` scripts are removed from `package.json`, as upstream does.
- Defaults match upstream's CLI: solution name is the hyphen-cased component name, library name is the solution name,
  alias is the component name, description is `<name> description`.
- Anything other than a plain `<%= %>` substitution (`<% if %>`, `<%- %>`, an unknown variable, a casing on a GUID) is an
  error naming the file, so a template that needs more than substitution is refused, not rendered wrongly.
- Binary files (by extension, or not valid UTF-8) are passed through untouched.

```powershell
cargo run --example upstream -- <spfx repo> webpart-react <location> "Hello World"
```

## Results

- **Same bytes as Microsoft's engine.** `webpart-react` was also rendered with the real `ejs` and `lodash` packages using
  upstream's own context rules, with the same three GUIDs. All 27 files are byte-identical to our output.
- **Builds.** The generated solution installed (1336 packages, Node 22.23.0), `heft build --production` passed with no
  errors or warnings, and `heft package-solution --production` produced a 38 KB `.sppkg` containing the manifest, bundle,
  strings, images and Teams icons. It was not deployed to a tenant.
- **All 20 templates** load, translate and scaffold with the defaults.
- **Five installed, built and packaged** (Node 22.23.0, `heft build --production` and `heft package-solution --production`,
  no errors or warnings in any build log):

  | Template | Kind | `.sppkg` |
  |---|---|---|
  | `webpart-react` (with the Forge overlay) | web part | 348 KB |
  | `webpart-noframework` | web part | 39 KB |
  | `extension-application-customizer` | extension | 8 KB |
  | `ace-generic-card` | Adaptive Card extension | 10 KB |
  | `library` | library | 5 KB |

  The branded `webpart-react` package is about nine times the size of the unbranded one (38 KB), because the logo is
  135 KB and the web part carries it twice (`welcome-dark.png` and `welcome-light.png`). A smaller welcome image would
  shrink it.
- Tests: `tests/upstream_templates.rs` (needs a clone of the repo as `../spfx`, or `FORGE_UPSTREAM_SPFX`; skipped
  otherwise) and 10 unit tests in `src/upstream.rs`.

## Overlay: your own images on top

`Overlay` (`overlay.json` plus the files it names) replaces files of an upstream template. Each key is a path in the
template's own syntax, matched at the end of a file's path; a rule that matches nothing in a template is fine, because
one overlay serves all of them. The templates repo has one in `layers/` that swaps the welcome images and the Teams
icons for the Forge logo and icons. For `webpart-react` it replaces four files.

## Releasing upstream templates

`build_release_from` accepts `ReleaseInput::Upstream { repo, template }` next to generator folders. The template is
translated, the overlay applied, a `manifest.json` generated, and the result archived as a generator named after the
template in a folder called `<template>.<spfx version>` (for example `webpart-react.1.24.1`). Clients only ever see our
format, so the store and the index did not change. `tests/upstream_release.rs` releases `webpart-react` and `library`,
installs them through the real `TemplateStore`, scaffolds from the installed copy, and checks that no tag is left and
that the welcome images and Teams icons are the overlay's. Rebuilding the same inputs gives the same index.

```powershell
cargo run --example release -- <out dir> <base url> --overlay <templates repo>\layers --upstream <spfx repo> webpart-react
```

## Differences from upstream, and things to know

- Upstream's `<%= %>` HTML-escapes values (`'` becomes `&#39;`). Values are inserted as typed here.
- The default alias is the raw component name, as upstream does, so `Hello World` gives the manifest alias
  `Hello WorldWebPart` (with a space). Upstream has the same default. It built fine, but a Pascal-cased alias may be
  nicer.
- The manifest `version` is the SPFx version and must be `x.y.z`, so a pre-release `spfxVersion` such as `1.24.0-beta.1`
  is refused for now.
- One component per run, as upstream. The multi-web-part `repeat` model of our own generators does not apply.
- Our own additions (the Forge logo, Teams icons, extra dependencies) are not part of the upstream templates. They would
  have to be an override on top.

## Not covered

- Installing and building the other 15 templates (the Copilot components, the other extensions and Adaptive Cards, the
  other web parts).
- Adding a component to an existing solution (the merge helpers).
- Fetching the upstream templates over the network. Releases are built from a local checkout.
- Older SPFx versions, which upstream does not have.
- A license review beyond the root `LICENSE`, and the notice obligations when copying templates into a release.
