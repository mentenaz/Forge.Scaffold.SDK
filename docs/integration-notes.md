# Future integration: Helm, the template engine and the analysis engine

> **Notes for later. Nothing here is built.** They come from reading `E:\tmp\helm-report5` (the analysis engine's static report of
> the Helm TUI, 18 November 2026, `forge-analysis 0.0.1`) and from what the template engine does today. Nothing in Helm's own
> source was read beyond what the report says, so statements about Helm below are marked "appears to".

## What the Helm report shows

- A Node and TypeScript project (React with Ink): 57 files analysed (both `src/` and the built `dist/`), 976 functions, 2,766
  facts, 5 dependencies and 4 dev dependencies, all from the registry. No install-time scripts (`prepublishOnly`, `build` and
  `dev` only). No flow of network data into code.
- It is a front end to other programs: 110 places start `gh`, `git`, `npm`, `open`, `xdg-open`, `clip`, `pbcopy` or `xclip`.
  It writes a helper script and changes its permissions (`buildAskPassEnv`, git credentials), deletes a file, and reads files.
- The report calls itself **incomplete**, which is the normal result for a real project: 27 places it cannot resolve (computed
  names, two functions used as values, one shell command), 119 imports it did not follow (mostly third-party packages), 629 calls
  whose target it did not see, and 9 dependencies whose own code it did not read.
- It is large: 4.5 MB of JSON and a 7,198-line Markdown file for 57 files.

## What Helm already does that a template flow needs

Helm appears to be a host of the kind `docs/custom-templates.md` describes: it already runs other programs, shows options before
an action, and shows progress. From the screen names in the report:

| Helm screen (from the report) | What the template engine offers |
|---|---|
| `CloneOptionsScreen`, `CreateRepoScreen` | the manifest's prompts (text, yes/no, list, choice) with per-field validation messages |
| `CloneProgressScreen` (reads a project's dependencies and appears to run `npm`) | the `apply` progress callback, then the `install` step the plan returns as a descriptor |
| `GhGateScreen` (a gate on the `gh` program being there) | the Node requirement check (`requires.node`, `NodeCheck`) |
| a list screen with versions or repositories | the version list (`list_versions`: installed, local, update available) |

## How the engine could be used from Helm

Helm cannot link a Rust crate. It starts programs, so the engine can be one:

1. **A small command-line program around the engine**, with JSON in and out and progress as JSON lines. Four commands would be
   enough: list the generators and versions, give a generator's form (the prompts), plan (answers in; files, warnings and post
   steps out; nothing written), and apply (writes; progress lines). Helm starts it the way it starts `gh`. The engine is pure
   logic, so this is a thin wrapper plus a stable JSON format, not new engine work. **Recommended first step.**
2. A native binding (napi) or WebAssembly build. More work, and the engine reads and writes folders, so it is not a good fit
   for WebAssembly. Not recommended.

Either way the host keeps the responsibilities in `docs/custom-templates.md`: show the plan and the warnings first, label custom
templates, run `install` only after the user agrees, and offer to install without scripts.

## A review step between "files written" and "install"

The analysis engine reads a folder and reports what its code does, without running it. The place for it is right after `apply`
and before `install`: plan, show the plan, apply, analyse the new project, show a short review, ask, then run the install.

What the Helm report says about making that work:

- **It needs a brief mode.** A prompt on a terminal cannot show 7,198 lines. The command line today prints only the coverage
  counts. What a review needs on one screen: install-time scripts (none or which), where the dependencies come from (registry, git,
  URL, local path), the effects by kind (starts programs, writes or deletes files, network out), anything of high severity, and how
  much it could not see.
- **"Incomplete" must not read as "safe" or "unsafe".** It is the usual result. The screen should say what was seen and what was not.
- **The dependencies are where the risk is, and they are not analysed** (9 of 9 here). A template that installs packages from the
  registry gets its real behaviour from them. The template engine's own warning (install scripts and non-registry dependencies in
  the generated `package.json`) is a small first check; the review does not replace reading dependencies.
- **Expected effects need context.** Helm starting `gh` and `git` is its job. A web project starting `curl` is not. A list of
  effects means more next to what the project is meant to be.
- **Built output doubles the report.** Helm's `dist/` mirrors `src/`, and the report notes that the same behaviour can appear
  twice. A freshly scaffolded project has no `dist/`, but a cloned repository often does.
- **It has now been tried on our own templates** (see the next section): no false alarms on what we generate, one kind of
  benign noise.

## Trial on our own generated projects (21 November 2026)

Run read-only: the analyser was built from the working tree of `E:\mentenaz\forge-analysis` (which had uncommitted work from
20 November; the repo was not changed) into a separate target folder, and pointed at four folders, each in a few seconds.

| Project | Files analysed | Effects | Data flows | Install-time scripts | Unresolved |
|---|---|---|---|---|---|
| Vite React (the published `vite-react-ts`, 15 files) | 3 | none | none | none | 5 |
| SPFx web part (official `webpart-react` 1.24.1) | 6 | none | none | none | 5 |
| SPFx web part (`yo`-derived 1.23.2) | 8 | none | none | none | 6 |
| **Control**: a project made to be hostile | 2 | 4 | 2 | `postinstall` | 4 |

- **Our projects come out clean.** Every script is "runs only when someone asks". The only `unresolved` entries are package
  scripts that call a tool (`vite`, `tsc`, `oxlint`, `heft`), which it cannot follow because they are not `node <file>` or
  `npm run <script>`. That is benign noise, and a catalog of well-known build tools would remove it. All dependencies come from
  the registry, and their own code is, as always, not analysed (9, 27 and 29 of them).
- **The control is caught.** It had a `postinstall` script, a git-URL dependency, and a script that downloads text and runs it.
  The report lists the install script ("runs at install"), the git dependency, the network request, the program start, the code
  built at run time and the file write, each with its line and "runs: immediate, install", and **two data flows**: the
  downloaded text reaching `execSync` and `eval`.
- **What it did not say:** the file write was reported without its target. The path was built from `process.env.HOME`, so it
  could not tell that it was the shell's start-up file.
- **The reports are small for a small project** (about 11 KB of Markdown and 50 KB of JSON for the Vite project), so the size
  problem seen on Helm is about big repositories. A brief mode is still needed for a terminal prompt.

What this changes in the plan: the review screen can reasonably say "nothing found" for our own templates and mean it; the
catalog of known commands is the first thing to improve; and the template engine's own warnings (install scripts, URL
dependencies, auto-run files) remain the cheap first check that needs no second program.

## Pending dependency: the analysis engine and its `TODO(forge-analysis)` marks

The analysis engine will be a dependency of Forge, used to validate what a template writes before an install. Until it is
finished, tested and confirmed, the template engine does not call it. The places where it will plug in are marked with a
**comment** tagged `TODO(forge-analysis)`; search the source for the tag. They are comments and not `todo!()` on purpose: a
`todo!()` panics when it runs, and any host that reached it would crash instead of carrying on.

| Where | What would happen there |
|---|---|
| `src/plan.rs`, next to the install warnings | the three cheap checks stay; the analysis runs **after `apply()`**, in the host, before `install` (a plan writes nothing, and the analyser reads a folder on disk) |
| `src/check.rs`, end of `check_with_manifest` | analyse each rendered variant; needs a scratch folder per variant, or an in-memory input in the analysis engine |
| `src/bin/forge-scaffold.rs`, `apply` and `check` | add the analysis summary to the JSON result under a new key, so existing hosts keep working |
| `src/lib.rs`, crate documentation | says the dependency is pending |

**Before the marks are replaced with real calls, the analysis engine should be confirmed to find these.** What the 21 November
trial already showed, and what it did not:

| Must be found | Trial result |
|---|---|
| install-time scripts (`preinstall`, `install`, `postinstall`, `prepare`) | found, and said to run at install |
| dependencies from a git repository, a URL or a path | found |
| network data reaching `eval` or a command (`execSync`) | found, as two flows with their lines |
| a file written without saying where | **not confirmed**: reported, but without the target (a path built from `process.env.HOME`) |
| files tools run by themselves (`.husky/`, `.vscode/tasks.json`, `.npmrc`, a dev container) | **not tested**: it reads JS, TS and `package.json`, and lists shell scripts as "not analysed". The template engine's own `AutoRunFiles` warning stays for this |
| shell scripts, and the commands in scripts that are not `node <file>` or `npm run` | **not followed** (listed as unresolved) |
| code inside dependencies | **not analysed**, by design |

And these, which are about being a dependency and not about what it finds:

- it builds and runs as a library dependency of Forge (today it is a workspace with `publish = false`), with a stable report
  format a host can rely on (it has a schema number; changes must be versioned);
- a **brief mode** that fits a terminal prompt (the Helm report was 7,198 lines);
- a catalog of well-known build tools (`vite`, `tsc`, `heft`, `oxlint`), so our own projects stop listing them as unresolved;
- a way to analyse without a written folder (or a documented decision to use a scratch folder);
- a test of its own for each row above, so "confirmed" means a test and not a one-off trial.

When those hold, replace each mark with the call, delete the mark, and update this section and `docs/custom-templates.md`.

## Suggestions for the analysis engine

For whoever finishes `forge-analysis`, so that it works well as a dependency of Forge next to the template engine. They come from
the Helm report, the trial above (four projects, one of them hostile) and the layout of the workspace. Where a point is a
guess about code that was not read, it says so. They are suggestions, not requirements, and none of them changes the template engine.

### A. Being a dependency

1. **A library entry point.** A public crate with `analyze(input, options) -> Report` and a `summary(&Report)`, instead of the
   pipeline living behind the command line (`forge_analysis_cli::analyze_folder` exists; a host should not depend on a CLI crate).
2. **Input that is not a folder.** The template engine holds a project **in memory** (`StagedFs`, `Plan.files`) before anything is
   written. If the analyser could take a list of `(path, bytes)` as well as a folder (the intake already makes a hashed snapshot),
   the review could happen **before** `apply()`, so no file is written until the user has seen it, and `check_template` could
   analyse each variant without a scratch folder. This is the single change that most improves the safety story.
3. **A brief mode.** A small, stable JSON summary: counts by effect kind, install-time scripts (with the trigger), dependency
   sources, flows, the top findings by severity with where they are, the coverage counts, and a duration. The Helm report was
   7,198 lines and the command line prints only coverage counts, so neither fits a terminal prompt.
4. **A stable, versioned report format** (there is a schema number: keep it, and say what changes bump it), and a promise that
   unknown fields can be ignored. Hosts in other languages will read the JSON.
5. **Compile cost and licences.** It compiled in about a minute and a half from cold because of `oxc`. A cargo feature for the heavy
   part, or a prebuilt binary the host can start, would keep Forge's own build lighter. Check that every dependency's licence
   fits a distributed product (`deny.toml` is there).

### B. Things it should find that the trial showed it does not (yet)

6. **Where a write goes.** The control wrote to `process.env.HOME + '/.bashrc'` and the report said only "writes files". Resolve
   paths built from `os.homedir()`, `HOME`, `USERPROFILE`, `APPDATA`, `path.join(...)` and classify sensitive destinations (shell
   start-up files, `~/.ssh`, startup folders, cron, git hooks, the registry) with a higher severity.
7. **Commands inside package scripts.** `"postinstall": "curl ... | sh"` is only "not followed" today. A small shell-command
   reader would classify: a well-known build tool (fine), a download piped into a shell, an encoded command (`base64 -d`,
   `powershell -enc`), `chmod +x`, `rm -rf`, `schtasks`, `crontab`, `reg add`.
8. **Files tools run by themselves that are not JavaScript.** `.husky/*`, `.vscode/tasks.json` (`runOn: folderOpen`),
   `.devcontainer/devcontainer.json` (`postCreateCommand`), `.npmrc` (a different registry), `.yarnrc.yml` (`yarnPath`, plugins),
   `.envrc`, `.pnpmfile.cjs` (a hook that runs on install, and it is JavaScript). The template engine warns about these by name today
   (`Warning::AutoRunFiles`); reading what is in them is the analyser's part. Shell and batch scripts are listed as "not analysed".
9. **Dependencies, after an install with scripts off.** Code inside dependencies is the biggest unseen part (9, 27 and 29 of them in
   the trial) and install scripts are the dangerous part of it. A second pass over `node_modules` that looks only at the dependencies'
   `preinstall`/`install`/`postinstall` scripts and `binding.gyp` would make this workable: install with `--ignore-scripts`, analyse,
   decide, then run the scripts (`npm rebuild`). The intake ignores `node_modules` today by design; this would be an explicit, bounded mode.
10. **Template placeholders.** A template folder (not a rendered project) holds `{__projectNamePascal__}` inside file names and
    identifiers, which is not valid JavaScript. Analysing the *rendered* output avoids it; if a template folder is ever analysed
    directly, a parse failure caused by a placeholder should not be reported as a finding.

### C. Signal quality

11. **A catalog of well-known tools** (`vite`, `tsc`, `heft`, `oxlint`, `eslint`, `prettier`, `jest`, `vitest`, `webpack`, `gulp`,
    `npx` of a registry package, ...). Today our own clean projects get five "shell command not followed" entries each. The
    catalog already exists for APIs (`node.toml`); this is its counterpart for commands.
12. **What is expected of this kind of project.** Helm starting `gh` is its job; a static web project starting `curl` is not. Let the
    caller pass a small policy (for example "a web front end: no process spawning, no network"), and report what falls outside it.
    The template engine could carry that policy in a manifest field later, so a template declares what its code is meant to do.
13. **A verdict next to the coverage.** "Incomplete" is the normal answer and says nothing about risk. Alongside it, a short
    ordered result (`nothing found`, `needs review`, `risky`) with the reasons, so a host can show one line and let the user open details.
14. **Built output.** In the Helm report `dist/` mirrors `src/` and doubles the findings. Detect the mirror, prefer the source, and say so.

### D. Trusting the tool itself

15. **Sanitise what it prints.** The report quotes text from the analysed code (it has a `Quoted` type for that). A terminal host
    will print it, so strip or escape control characters, ANSI escape sequences and bidirectional overrides ("Trojan Source").
    This is a guess about code that was not read: confirm it with a test that puts those characters in an analysed file.
16. **Same output on every system.** Paths with `\` and `/`, CRLF and LF, and the snapshot hash should come out identical on
    Windows and Linux. The working tree showed line-ending warnings on files that are tracked.
17. **Two test corpora from the template side**, so "confirmed" is a test and not a trial: every template Forge ships, scaffolded,
    must come out clean (no effects, no flows, no install-time scripts); and a set of hostile projects (the control, plus one per
    row of the table above) must each be flagged. The template engine's `tests/hostile_templates.rs` is a source of cases.
18. **Keep the fuzz and robustness tests** (the history mentions them): the analyser reads hostile input by design, and it must not
    panic, hang or use unbounded memory on it. A time limit as well as the size limits would help.

### E. Something only the two engines can do together

19. **Diff two versions of a template.** The store downloads template updates. Analyse the old and the new version and show what
    changed in behaviour ("this update adds a network request and a postinstall script") before the update is installed. This
    needs stable fact identities across runs (there are unique fact IDs already) and a `diff(report_a, report_b)`.

## Order I would take it in

1. The JSON command-line program around the template engine (the small, useful step; Helm could then offer "new project").
2. Run the analysis engine on the generated SPFx and Vite projects and read the false alarms. It is read-only, so it is safe.
3. A brief mode in the analysis engine (the five items above).
4. The review screen in the host, between apply and install.
5. A tool that makes a template from an existing project, and a one-call check of a template folder (both listed as not built in
   the README).

Keep the three pieces separate: the template engine stays pure logic with no heavy dependencies, the analysis engine stays a
separate workspace, and the host (Helm or the Forge panel) is where the two meet and where the user's consent is asked.

## Questions for the owner

- Is Helm meant to start new projects from templates, or only to clone existing repositories?
- Should the review be required for custom templates only, or also for the templates Forge publishes?
- Where should the brief report be defined: in the analysis engine, or built by the host from the full report?
