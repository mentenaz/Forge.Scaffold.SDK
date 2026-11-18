# Adding to an existing solution

The host asks the engine what is in a folder, then does one of two things:

```text
detect_spfx(folder) ──► Some(solution)  ──►  plan_add()  ──►  show the plan  ──►  apply_add()
                   └──► None            ──►  plan()       ──►  apply()          (a new solution)
```

## Detecting a solution (`detect_spfx`)

The solution's own files decide. `.yo-rc.json` is read when it is there, but only as a hint.

- A folder is an SPFx solution when it has `package.json`, `config/config.json` and `config/package-solution.json`, and
  `package.json` has an `@microsoft/sp-*` or `@microsoft/spfx-*` package. The nearest folder that does wins, so it works
  from any folder or file inside the solution.
- **Version:** from the SPFx packages (`sp-core-library` first). Build tooling a patch ahead of the runtime (for example
  `sp-build-web` 1.20.2 next to `sp-core-library` 1.20.0) is normal and does not warn; a different minor version does.
- **Build system:** `gulpfile.js` or `sp-build-web` means gulp, `config/rig.json` or the heft packages mean heft.
- **Framework:** `react` when it is a dependency, otherwise the marker's `template`.
- Why not the marker alone: the official SharePoint/spfx CLI does not write `.yo-rc.json`, it can be stale after hand
  edits, and a marker the engine itself wrote would only exist for solutions the engine made.

An unreadable `package.json` in a folder that has the solution files is an error, not a silent miss.

## Planning and applying an add (`plan_add`, `apply_add`)

`plan_add` writes nothing. It returns every file with its outcome (`New` or `Merged`), the new content, the previous content
of edited files (for a diff), the files it left alone (`kept`), and warnings. `apply_add` writes it.

How a generator is added is part of its manifest (`add.mode`):

| Mode | For | What runs |
|---|---|---|
| `items` (default) | our own generators | only the `repeat` folders, rendered for the new names, and the `merge`s that are `per` a list (the `config/config.json` entries of a web part). Needs the web part list, not the solution name. |
| `render` | the official SPFx templates | the whole template is rendered and written into the solution file by file (below). |

Rules for both:

- Nothing is ever deleted or overwritten.
- A file the add would create that already exists is an error (`ExistsInSolution`), and nothing is written.
- The generator's SPFx **minor** version must be the solution's. Another patch (1.20.2 solution, 1.20.0 templates) only
  warns; another minor version is refused (`Incompatible`). A manifest can also name its build system (`add.buildSystem`):
  a heft template is refused for a gulp solution.
- An existing file is only edited by a merge. The edit keeps the file's indent (tabs or spaces), line endings (CRLF or LF),
  final newline, byte order mark and key order, so only the new entries show in a diff. A merge that adds nothing leaves the
  file byte for byte as it was.
- Files with comments or trailing commas are refused with a clear message, not rewritten.
- `apply_add` first checks that nothing changed since the plan (`ChangedSincePlan`). Each edited file is written beside
  itself and renamed over it. If a write fails, created files and folders are removed and edited files are put back.
- `.yo-rc.json` is never written.

```powershell
cargo run --example add -- <generator folder> <folder in the solution> HelloWorld,OtherPart --apply
cargo run --example add -- --upstream <spfx repo> webpart-react <folder in the solution> "Other Part" --apply
```

Exit code 3 means the folder is not inside an SPFx solution (scaffold a new one). Without `--apply` it only prints the plan.

### Render mode: the official 1.24.1 templates

An official template renders a whole solution around one component, so an add renders all of it and decides per file:

| The file in the solution | What happens |
|---|---|
| does not exist | created (`New`) |
| exists and is identical | skipped |
| exists, differs, and the manifest lists a merge for it | merged (`Merged`) |
| exists, differs, no merge | **kept** as it is, and listed in `AddPlan::kept` and a warning |

`add.component` names the prompt that names the component (`componentName`). If a template file whose path contains that
name already exists in the solution, the component is already there and the add is refused, even if everything else
matches. `add.inherit` makes the solution answer its own `libraryName` and `solutionName` (from `package.json`), which are
then not asked and not checked against the naming rules for new solutions.

The merges (`crate::merge`) only ever **add**. A value the solution already has is never changed:

- `package.json`: dependencies and devDependencies the solution lacks, after the existing ones. A package already in either
  section keeps the solution's version. An `@microsoft/*` package on another **minor** version stops the add.
- `config/config.json`: `bundles`, `localizedResources` and `externals` entries the solution lacks.
- `config/package-solution.json`: the solution's own metadata stays. Every component brings a feature with a fresh id and
  nothing in it; those are skipped when the solution already has a feature. A feature that provisions element files
  (extensions) joins the feature that already does, so all extensions share one feature and one set of element files; if
  there is none yet, it is added.
- `config/serve.json`: `serveConfigurations` entries the solution lacks.
- `sharepoint/assets/elements.xml` and `ClientSideInstance.xml` (extensions): the template's elements are added inside the
  solution's own `<Elements>`, as text, so everything already in the file stays. An element whose component id is already
  there is not added twice. Without this, the second extension (of any kind: they all write `elements.xml`) would be kept
  out of the file and never provisioned, while still getting a feature. A warning is given when two elements share an `ID`,
  or two fields share a `Name` (each field customizer template ships a sample column called `SPFxPercentage`).

`add.exclusive` lists files that register a component of their own. If one already exists in the solution and differs, the
add is refused: keeping it would leave the new component unregistered. The Copilot templates use it for
`config/copilot-agent.json`, so a solution takes one Copilot component and a second is refused with a clear message.

Where this differs from the official CLI (read from its code):

- Its `config.json` and `serve.json` merges let the template's scalars win (`port`, `https`, `initialPage`), which would
  overwrite a developer's own tenant URL. Here the solution's value always stays.
- Its `package-solution.json` merge appends the template's feature whenever its id is new, so each added component would add
  an empty feature. Here an empty one is skipped. This is the policy to flip if a solution should get a feature per component.
- It preserves existing files that differ, as here, but it overwrites a differing binary file. Here nothing existing is replaced.

## What was verified

**SPFx 1.20.0, gulp (our generator, `items` mode)**

- `tests/add_spfx.rs` scaffolds a solution, detects it from a file deep inside, and adds a second web part. This matches what
  `yo` was measured to do (`spfx-add-findings.md`): 11 new files (the 9-file component folder and two Teams icons), and
  `config/config.json` is the only file edited. Every other file is byte-identical, no line of the original `config.json` is
  rewritten, a repeat add is refused, and planning writes nothing.
- Built for real under Node 18.20.8 (`gulp build`, `gulp bundle --ship`, `gulp package-solution --ship`): all exit 0, no errors
  or warnings. The `.sppkg` (122 KB) holds both web parts.

**SPFx 1.24.1, heft (the official templates, `render` mode)**

- `tests/add_heft.rs`: a second `webpart-react` component added to a `webpart-react` solution gives 11 new files and a single
  edit, `config/config.json`; everything else is byte-identical, the solution keeps one feature, the same component again is
  refused, a third still goes in. A `webpart-react` component added to a `webpart-noframework` solution also merges
  `package.json`: the React packages are added after the existing ones, every existing entry keeps its version and order, and
  no original line of the file is rewritten. The solution's own `.eslintrc.js`, `.npmignore` and README are kept.
- Both results were installed, built and packaged for real (Node 22.23.0, `heft build --production`,
  `heft package-solution --production`): all exit 0, no errors or warnings in the build logs. Each `.sppkg` (47 KB) holds both
  web parts' bundles, strings and Teams icons. Nothing was deployed to a tenant.
- Unit tests cover the four merges, formatting preservation (4-space indent with CRLF, a byte order mark, no final newline),
  the refusal cases, the per-file rules, the component guard, inherited answers, the changed-file check and rollback.

## Not covered yet

- Real builds of extensions, libraries, Adaptive Card extensions and Copilot components added to a solution: all 17 types
  plan cleanly against a real solution and the cumulative case is tested (`tests/add_heft.rs`), but the install-and-build
  check has not been recorded yet (only web parts were built).
- A React component added to a solution without React leaves that solution's own lint and TypeScript settings in place
  (they are reported as kept). It built here, but it is the case to watch.
- JSON files with comments (they need a comment-aware editor, which is not built).
- Gulp-based solutions with the official templates (those are heft only).
- A marker file. It is optional for add; it would only record which template made a solution, and matters for generators
  with no recognisable files (the full-stack and Vite scaffolds).
