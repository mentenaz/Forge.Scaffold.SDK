# Roadmap: the rest of the template engine

Scope: the template engine (`forge-scaffold`), its data (the templates repo), and how it is published. **Not** in scope: the
analysis engine (`docs/integration-notes.md` covers it) and the host's own work in Helm and the Forge panel, except where the
engine has to give them something.

Sizes are rough, not promises: **S** under an hour of focused work, **M** a session, **L** several sessions.
"Owner" means a decision or an action only the project owner can take.

## Where it stands (21 November 2026)

Works and is tested (298 tests, Windows only so far): scaffolding from a folder, a pack, the official SPFx templates and the
create-vite templates; adding SPFx components to an existing solution; the templates store, releases and the live index
(26 archives); comment-preserving JSON edits; Node version checks; `check_template`, `create_template`, the JSON program
`forge-scaffold`, the authoring guide; the protections and warnings in `docs/custom-templates.md`.

Not built: conditional prompts, adding to a project that is not SPFx, the NuGet source, remote user-supplied templates, the
pack registry and the marker file, the generators `fullstack` and `backend-api`, and any test run on Linux or macOS.

## Phase A: foundations (no decisions needed; start here)

| # | Work | Why | Size | Done when |
|---|---|---|---|---|
| A1 | **CI for the engine.** *Built, not yet run on GitHub* (`.github/workflows/ci.yml`): `cargo test` and `cargo clippy -D warnings` on Ubuntu, Windows and macOS, checking out the SPFx and Vite forks so the real-template suites run too. Before the first run it was checked locally: the whole project, tests included, compiles and is clippy-clean for Linux and macOS targets, and the real-template suites pass against the same sparse checkouts CI makes. | The repo has no CI at all, so nothing has ever run off this machine. It answers the Linux and macOS question without a VM (the Unix symlink and case-collision tests finally run) and guards every later change. | S | green on the three systems; any failures listed |
| A2 | *(done)* **Mark the public enums `#[non_exhaustive]`** (`Warning`, `ScaffoldError`, and the other enums a host matches on). | This round added variants and would have broken any host that matched them exhaustively. Doing it before a host depends on the crate costs nothing; doing it later is a breaking change. | S | a host `match` needs a `_` arm; changelog says so |
| A3 | *(done)* **Conditional prompts**: an optional `when` on a prompt. A prompt whose condition fails is not asked, is not validated, and has no answer. A `when` may only refer to an earlier prompt. | A `fullstack` generator ("which database?" only if "use a database" is yes) and a "framework, then variant" flow need it. Everything else in the manifest already has `when`. | M | manifest check rejects a forward or missing reference; `check_template` renders the hidden and shown cases; the form JSON carries the `when`; schema updated |
| A4 | **Make the JSON program a host-ready v1.** Only if Helm (or another host that is not written in Rust) is to use the engine: the Forge panel is Rust and links the crate directly, supplying the `Fetch` trait itself. Add a protocol version to every result; document it (`docs/host-protocol.md`); add `install-archive` (the host downloads an archive and its index entry, the program verifies the sha256 and installs atomically) and a command that lists generators. | The program cannot download (the crate opens no connections), so a Node host needs a way to get templates installed. The format is a first draft and should be tried on a real screen before it is frozen. | M | Helm (or a test host) installs, lists, forms, plans and applies using only the program. **Waiting on:** whether Helm is meant to use the engine at all |

## Phase B: generators (the data)

| # | Work | Size | Notes |
|---|---|---|---|
| B1 | **Republish `vite-react-ts`** (revision 2, the Forge page): a build-only run on GitHub first, then live. | S | the live server still has revision 1 with Vite's logo and demo page. Needs your go-ahead |
| B2 | **All 16 create-vite templates** (decided: the fork is ours, so use it all): `lit`, `preact`, `qwik`, `react`, `solid`, `svelte`, `vanilla`, `vue`, each in TypeScript and JavaScript. For each: scaffold, install, build and lint on Node 22; add `<template>:` overlay rules (a Forge page in that framework's own idiom, README, removals) so no Vite or framework branding remains; add to the publish list. `react-ts` is done. | L in all (about S each, 15 times; the pages differ per framework) | start with `vue-ts` and `vanilla-ts`, then the rest in batches, publishing each batch only after it builds |
| B3 | **Tidy the placeholders.** The empty `generators/vite-react`, `fullstack` and `backend-api` folders, and the sample pack whose icon is a truncated placeholder; diff the `yo` captures against a clean `yo` run (never done). | S to M | |
| B4 | **`fullstack` and `backend-api` generators.** Decided: the front end is Vite. The back end is my choice, and it has to be one the engine can cover. Plan: **first Node and TypeScript (Fastify)**, because the engine's `install` step, the Node check and the checks already cover it, so no engine change is needed. Then a `backend` choice (needs A3) adds **ASP.NET Core** (from B5) and **Rust (axum)**; those need fixed follow-up steps the host can run, `dotnetRestore` and `cargoFetch`, added to the short list of steps a manifest may name (the engine still runs nothing). A database comes through `composeUp`. | L | needs A3; the back end choice is open to change |
| B5 | **.NET templates, from Microsoft's own** (decided: it must be available, there are none of ours). A reader for the `dotnet new` format, converted at release time like create-vite, with the MIT notice. Sized from the packs installed with the SDK here (21 November 2026): the web pack has 14 templates (`webapi`, `webapiaot`, `web`, `webapp`, `mvc`, `blazor`, `blazorwasm`, `grpc`, `worker`, `razorclasslib`, ...) and the common pack 17 (`console`, `classlib`, `xunit`, `nunit`, `mstest`, ...), in C#, F# and VB. Both are MIT, Copyright Microsoft; the web templates also carry bootstrap and jQuery with their own notices. What the reader needs: `template.json` (JSON with comments; `jsonc.rs` can read it), `sourceName` renaming, **symbols** of five kinds (parameter, computed, generated, bind, derived), **conditions** (an expression language in `computed` symbols and in `sources` modifiers), **`#if` preprocessing inside source files** (70 of 377 text files in the web pack), `guids`, and `postActions` mapped to fixed steps (`dotnetRestore`). The packs are `.nupkg` (zip). | XL | the work is large but checkable: the SDK is installed (10.x, and 6, 7, 8 packs), so each converted template can be compared **byte for byte against what `dotnet new` itself writes**, as was done for the SPFx and Vite templates. Start with `webapi`, `console`, `classlib`, `worker`; the UI templates later |

## Phase C: reach (each needs a decision first)

| # | Work | Size | Notes |
|---|---|---|---|
| C1 | **Adding to a project that is not SPFx.** Decided that it is wanted: a Vite template that can only create a project cannot add another piece to it later. The manifest declares how to recognise its kind of project (a marker file with a key, or a dependency in `package.json`) and what an add creates and merges; `plan_add` stops taking an `SpfxSolution` and takes a small project description. The SPFx behaviour must stay byte for byte the same, and the existing add tests prove it. First kind: a Vite project. **Waiting on:** one concrete example of "add another" (see the questions below) | L | today detection for other projects is marker-file based (`.yo-rc.json`); SPFx is read from its files |
| C2 | **Community templates through pull requests** (decided direction: a registry, not arbitrary remote indexes). A contributor opens a PR with a template folder; automation validates it; a maintainer reviews and merges; the release workflow publishes it and records who submitted it (the PR and commit) in the index. Pieces: a registry repo (or a `community/` folder in the templates repo) with a folder layout, `CONTRIBUTING.md`, a PR template and `CODEOWNERS`; a PR check that runs `forge-scaffold check` (exists now), the hostile-template corpus, a size and licence check, and later the analysis engine (the `TODO(forge-analysis)` marks); and **a contributor terms flow** (a DCO sign-off or a CLA, the licence a template must carry, provenance for anything derived from others' work, no secrets or trademarks, and a takedown process). **The terms still need to be discussed**: I can draft the options. Anything not through the registry stays a custom template, trusted by its user. | M for the automation, plus the terms | the PR check can be built without the terms; the terms decide what a contributor must agree to |
| C3 | **Official SPFx templates on gulp solutions.** | – | not planned: Microsoft ships them for heft only |

## Phase D: release hygiene

| # | Work | Size |
|---|---|---|
| D1 | A versioning policy for the crate and for the JSON protocol, and a `CHANGELOG`. | S |
| D2 | A public-API audit (a lot of modules are `pub` that a host does not need) and a rustdoc pass with examples. | M |
| D3 | Wire the crate into the fork as a git dependency (owner), using `examples/host_flow.rs` as the model. | owner |
| D4 | Server housekeeping: delete the unused `SFTP_PATH` secret; decide whether to keep the FTPS fallback and the `forgetemplates` account; re-run `restrict-key.sh` after any key change in cPanel. | owner, S |

## In parallel, owner only

- Legal review of the trust statement in `docs/custom-templates.md` and the `BRANDING.md` and `LICENSING.md` wording.
- The `yo` generator licence question (SharePoint/sp-dev-docs discussion 11052), and reading the SPFx repo's contribution terms.
- Whether to say in the public repo that the artwork was made with AI tools.
- A run of the test suite on the Linux VM (A1 makes this a backup).
- Host work in Helm and the panel: show the plan and warnings before an install, label custom templates, offer an install without
  scripts, handle the newer errors. It depends on A2 and A4.
- The analysis engine (separate): see `docs/integration-notes.md`.

## Suggested order

1. **A1** (CI), **A2** (`non_exhaustive`): small, and everything after is safer for them.
2. **A3** (conditional prompts): unlocks the generators that need it.
3. **B1, then B2** (republish Vite; `vue-ts` and `vanilla-ts`): visible progress that needs no decision.
4. **A4** (the JSON program as a host-ready v1), together with a first real try in Helm.
5. The decisions below, then **C1**, **B4**, **B5**, **C2** in whatever order the decisions make useful.

## Decisions (21 November 2026)

| # | Decision | Effect on the plan |
|---|---|---|
| 1 | Adding to existing projects is wanted beyond SPFx, Vite first | C1 is in; it needs one concrete example |
| 2 | `fullstack`: the front end is Vite, the back end is my choice and must be covered | B4: Node and TypeScript first, then a `backend` choice with ASP.NET Core and Rust, which need two more fixed follow-up steps |
| 3 | .NET templates must be available, from Microsoft's own | B5 is in, sized XL, verified against `dotnet new` |
| 4 | A pull-request registry with automated validation and a contributor terms flow; the terms to be discussed | C2 is redirected from "remote indexes" to a registry; the automation can start before the terms |
| 5 | (a question) why is Helm mentioned | the Forge panel is Rust and links the crate; Helm only matters if it is to use the engine (A4) |
| 6 | All 16 create-vite templates, because the fork is ours | B2 widened to all of them |

## Still open

Answered on 21 Nov 2026:

- **Item 1 (add another):** not needed for now. Adding a second piece to a Vite project risks overwriting what is there, so C1 is parked until there is a case that is safe by construction.
- **Item 2:** Node and TypeScript (Fastify) is the first back end, until changed.
- **Item 5:** the Forge panel is the only host. It will be its own panel in the Zed fork once the back end is built, so A4 (JSON program for non-Rust hosts) stays parked.

Open:

- **Item 3:** which .NET version(s) and templates. Undecided; nothing to build until it is.
- **Item 4:** contributor terms. The draft Forge Terms of Use (v0.1, section 9) will live in the Zed fork and is not final; the open points are DCO or CLA, the licence, provenance and takedown. The registry (C2) waits for that.
