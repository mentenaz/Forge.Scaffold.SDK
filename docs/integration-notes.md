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
- **It has not been tried on our own templates.** The SPFx and Vite generated projects should be run through it to see how many
  alarms it raises on templates we know to be clean. If it raises many, the catalog needs tuning before the review is shown to users.

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
