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
- A component that is already in the solution is refused as a whole, naming it (`ComponentExists`: `'Hello' is already in
  this solution ('src/Types/IHello/IHelloProps.ts' exists), so nothing was added`), and nothing is written, not even the
  other names of the same request. The check ignores letter case (`add.rs`, `existing_like`), so `hello` and `Hello` are the
  same component on a case-sensitive file system too, and the message says when the files differ only by case. Any other
  file the add would create that already exists is an error (`ExistsInSolution`).
- The generator's SPFx **minor** version must be the solution's. Another patch (1.20.2 solution, 1.20.0 templates) only
  warns; another minor version is refused (`Incompatible`). A manifest can also name its build system (`add.buildSystem`):
  a heft template is refused for a gulp solution.
- An existing file is only edited by a merge. The edit keeps the file's indent (tabs or spaces), line endings (CRLF or LF),
  final newline, byte order mark and key order, so only the new entries show in a diff. A merge that adds nothing leaves the
  file byte for byte as it was.
- Files with comments or trailing commas are edited as text (`src/jsonc.rs`): every comment, blank line and trailing comma stays, and new entries copy the file's indent and line endings. Only a file that is not JSON at all is refused. (`package.json` is not special-cased, npm itself cannot read comments in it.)
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
- `package.json` with `package-json-scripts`: everything `package-json` does, and also the `scripts` the solution lacks. A
  script or dependency the solution already has keeps its value, and the plan carries a sentence for each one that differs
  from the template ("package.json already has the script 'serve' as ... and it was kept"). Used by the fast-serve add-on.
- `gulpfile.js` with `spfx-gulpfile`: the template's blocks, written `/* name */` ... `/* end of name */`, are added in front of
  the `build.initialize(...)` line, once. A block whose opening marker is already there, or whose first `require`d package
  the file already uses, is left alone, so a gulpfile patched by the official `spfx-fast-serve` counts as done. Text based:
  the rest of the file, its quotes and its line endings stay. Without a `build.initialize(` line the merge is refused, and
  the message carries the block to paste by hand.
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

`add.anySpfxVersion` is for an add-on whose own version has nothing to do with SPFx's (fast-serve): without it the
generator's `version` must be the solution's SPFx minor. `add.inherit` also takes `spfxMinor`, the minor part of the
solution's SPFx version as digits (`21` for 1.21.1), for a package whose version follows SPFx's, such as
`spfx-fast-serve-helpers` (`~1.{__spfxMinor__}.0`). A generator that only adds to a solution has no `folderFrom`; a real
`plan()` of it is refused, and `check_template` dry-runs it with a placeholder folder name.

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
- **The other component types.** All 17 non-web-part templates plan cleanly against a real solution, and the first of each
  kind matches what `yo` was measured to do (a customizer edits `config.json`, `package-solution.json`, `serve.json` and
  `package.json`; a form customizer `config.json`, `serve.json` and `package.json`; an Adaptive Card extension `config.json`
  and `package.json`; a library `config.json`). Three solutions were built up from a `webpart-react` base, adding component
  after component: one with eight extensions (application customizer, three field customizers, two form customizers, list
  view command set, search query modifier), one with five Adaptive Card extensions and a library, and one with a Copilot
  component. All three installed, built and packaged (Node 22.23.0, `heft build --production` and
  `heft package-solution --production`, exit 0, no errors or warnings). The extensions package (80 KB) has nine bundles and a
  single feature folder whose `elements.xml` provisions all six custom actions and fields and whose `ClientSideInstance.xml`
  holds the three component instances. The Adaptive Card and library package (72 KB) has seven bundles, the Copilot package
  (101 KB) two. Nothing was deployed to a tenant.
- Doing that found a real bug, now fixed and tested (`tests/add_heft.rs`): the second extension used to keep the existing
  `elements.xml`, so it was never provisioned while still getting a feature of its own.
- Unit tests cover the merges, formatting preservation (4-space indent with CRLF, a byte order mark, no final newline),
  the refusal cases, the per-file rules, the component guard, inherited answers, the changed-file check and rollback.

## Not covered yet

- **Same name in another letter case on Linux (and macOS with a case-sensitive volume): written, not yet run there.** The
  development machine is Windows, whose file system is case-insensitive, so `exists()` already caught `hello` against
  `Hello` and the new `existing_like` check was only exercised by unit tests that list directories (they pass on any
  system, since the helper reads directory entries and compares them without regard to case). To check it on Linux:
  1. `cargo test add::` and `cargo test --test add_spfx` should pass as they are.
  2. Scaffold a solution with a web part `Hello`, then add `hello` and `HELLO` to it (`cargo run --example host_flow --
     --templates <folder> --generator spfx-webpart --folder <solution> --answer webparts=hello`). Each should stop with
     `'hello' is already in this solution (...)` and write nothing. Note that the name tokens normalise case (on
     Windows `hello`, `Hello` and `HELLO` all produced `IHelloProps.ts` and `src/webparts/hello/...`), so these names
     give identical paths and the older exact check would have caught them on Linux as well. `existing_like` matters
     where a generated path differs only in case from an existing one (a solution made by hand, or a generator whose
     paths keep the case of the name), and there is no real example of that yet, so this step may only show that nothing
     regressed.
  3. If a name goes through that should not, the place to look is `existing_like` in `src/add.rs` and the two places that
     call it (the loop over `item_paths` in `plan_add` and the component guard in `plan_add_render`). The first path the
     add finds is the one reported, and the name in the message is the one the user typed.
- A React component added to a solution without React leaves that solution's own lint and TypeScript settings in place
  (they are reported as kept). It built here, but it is the case to watch.
- Gulp-based solutions with the official templates (those are heft only).
- A marker file. It is optional for add; it would only record which template made a solution, and matters for generators
  with no recognisable files (the full-stack and Vite scaffolds).
