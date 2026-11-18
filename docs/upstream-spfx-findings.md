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
- **All 20 templates installed, built and packaged** (Node 22.23.0, `heft build --production` and
  `heft package-solution --production`). Every step exited 0 and no build log has an error or warning line. All were
  scaffolded with the Forge overlay and the component name `Hello World`; the first five were built in an earlier
  round and `webpart-react` was rebuilt after the images were shrunk.

  | Template | Kind | `.sppkg` |
  |---|---|---|
  | `webpart-react` | web part (with the Forge overlay) | 102 KB |
  | `webpart-noframework` | web part | 39 KB |
  | `webpart-minimal` | web part | 23 KB |
  | `extension-application-customizer` | extension | 8 KB |
  | `extension-fieldcustomizer-minimal` | extension | 9 KB |
  | `extension-fieldcustomizer-noframework` | extension | 9 KB |
  | `extension-fieldcustomizer-react` | extension | 9 KB |
  | `extension-formcustomizer-noframework` | extension | 8 KB |
  | `extension-formcustomizer-react` | extension | 8 KB |
  | `extension-listviewcommandset` | extension | 8 KB |
  | `extension-search-query-modifier` | extension | 8 KB |
  | `ace-data-visualization` | Adaptive Card extension | 10 KB |
  | `ace-generic-card` | Adaptive Card extension | 10 KB |
  | `ace-generic-image-card` | Adaptive Card extension | 14 KB |
  | `ace-generic-primarytext-card` | Adaptive Card extension | 10 KB |
  | `ace-search-card` | Adaptive Card extension | 10 KB |
  | `library` | library | 5 KB |
  | `copilot-component-minimal` | Copilot component | 10 KB |
  | `copilot-component-noframework` | Copilot component | 90 KB |
  | `copilot-component-react` | Copilot component | 67 KB |

  The branded `webpart-react` package was 348 KB at first, about nine times the unbranded one (39 KB): the logo was
  135 KB and the web part carries it twice (`welcome-dark.png` and `welcome-light.png`), and the Teams colour icon
  was 65 KB. The templates repo now ships both as 256-colour palette PNGs (welcome image 38 KB, Teams icon 14 KB), which
  brings the package to 102 KB. It was rebuilt and packaged again after the change, with no errors or warnings, and the
  package step accepted the palette icon. At 220 px the welcome image would be about 23 KB, but it looks clearly soft
  when the web part stretches it to 420 px, so it stays at 300 px.
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

- Running any of the built solutions: only install, build and package were checked, not a workbench or a tenant.
- Adding extensions, libraries and Adaptive Card extensions to an existing solution. Adding web part components to one
  (including a React web part into a no-framework solution) is built and verified: `docs/add-to-solution.md`.
- Fetching the upstream templates over the network. Releases are built from a local checkout.
- Older SPFx versions, which upstream does not have.
- A license review beyond the root `LICENSE`, and the notice obligations when copying templates into a release.
