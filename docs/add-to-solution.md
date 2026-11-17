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

An add runs only the per-component part of a generator: every `repeat` folder rendered for the new names, and every
`merge` that is `per` a list (the `config/config.json` entries of a web part). The solution-level parts (`include`,
`parts`, the other merges, the post steps) are not run. So an add needs the web part list, not the solution name.

`plan_add` writes nothing. It returns every file with its outcome (`New` or `Merged`), the new content, the previous content
of edited files (for a diff), and warnings. `apply_add` writes it.

Rules:

- Nothing is ever deleted.
- A file the add would create that already exists is an error (`ExistsInSolution`), and nothing is written.
- The generator's SPFx **minor** version must be the solution's. Another patch (1.20.2 solution, 1.20.0 templates) only
  warns; another minor version is refused (`Incompatible`).
- An existing file is only edited by a merge. The edit keeps the file's indent (tabs or spaces), line endings (CRLF or LF),
  final newline and key order, so only the new entries show in a diff.
- Files with comments or trailing commas are refused with a clear message, not rewritten. (Keeping comments needs a
  comment-aware JSON editor, which is not built.)
- `apply_add` first checks that nothing changed since the plan (`ChangedSincePlan`). Each edited file is written beside
  itself and renamed over it. If a write fails, created files and folders are removed and edited files are put back.
- `.yo-rc.json` is never written. `yo` records its last answers there after an add; the build does not read them.

```powershell
cargo run --example add -- <generator folder> <folder in the solution> HelloWorld,OtherPart --apply
```

Exit code 3 means the folder is not inside an SPFx solution (scaffold a new one). Without `--apply` it only prints the plan.

## What was verified (SPFx 1.20.0, gulp)

- `tests/add_spfx.rs` scaffolds a solution with our 1.20.0 generator, detects it from a file deep inside, and adds a
  second web part. This matches what `yo` was measured to do (`spfx-add-findings.md`): 11 new files (the 9-file component
  folder and two Teams icons), and `config/config.json` is the only file edited. Every other file is byte-identical, no
  line of the original `config.json` is rewritten, a repeat add is refused, and planning writes nothing.
- Built for real: the solution with the added web part was installed and built under Node 18.20.8
  (`gulp build`, `gulp bundle --ship`, `gulp package-solution --ship`, all exit 0, no errors or warnings in the build log).
  The `.sppkg` (122 KB) contains both web parts' bundles, strings and Teams icons. It was not deployed to a tenant.
- Unit tests cover formatting preservation (4-space indent with CRLF), refusal cases, the two-phase apply, the changed-file
  check and rollback after a failed write.

## Not covered yet

- Heft-based solutions and the official 1.24.1 templates. Those render the whole solution, so an add needs the four merge
  helpers upstream has (`package.json`, `config.json`, `package-solution.json`, `serve.json`); only the `config.json`
  style of merge exists. Whether upstream's `package-solution.json` merge adds one feature per added component (it appends
  the incoming feature by id) was read from its code and not run.
- Extension, library and Adaptive Card components (they also edit `package.json`, `package-solution.json` and `serve.json`).
- JSON files with comments.
- A marker file. It is optional for add; it would only record which template made a solution, and matters for generators
  with no recognisable files (the full-stack and Vite scaffolds).
