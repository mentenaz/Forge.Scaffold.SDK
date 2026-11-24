# Importing a package into an SPFx solution

*Since 0.4.0-alpha.1. A first slice: what is built and what is not is listed at the end. The names and fields below are what
the panel can build against; an alpha may still change them, and the changelog says so when it does.*

A **package** is a folder: a `manifest.json`, a `package.json` that lists what it needs, and source. Importing it copies the
source into an existing SPFx solution, merges the needs into the solution's `package.json`, and says what it found as data.
The engine runs no command, makes no connection, and never deletes or overwrites.

```text
acme-data-services/
  manifest.json      id, version, "prompts": [] and the "import" section below
  package.json       dependencies, peerDependencies (devDependencies are the author's own tools and are never added)
  src/               ctx.ts, errors.ts, types/ (the core), ItemOps/, ListOps/ (modules)
  tests/ dist/ ...   never copied: only what the manifest names is read
```

`tests/fixtures/acme-data-services` in this repository is a made-up package of this shape, used by `tests/import_package.rs`.

## The manifest's `import` section

```json
"import": {
  "kind": "service",
  "root": "src",
  "target": "src/services/acme-data-services",
  "core": ["ctx.ts", "errors.ts", "types"],
  "modules": [ { "name": "ItemOps", "title": "Item operations", "paths": ["ItemOps"] } ],
  "provided": ["rxjs"],
  "notes": ["Import from the module folder: ..."]
}
```

* `kind`: `service` (default; lands in `src/services/<slug>/`), `ui` (`src/components/<slug>/`) or `library` (no files, only the
  needs: a recipe such as "add axios").
* `root`: where the source is in the package (default `src`). `core` and the modules' `paths` are relative to it, and they land
  below the target **without** it, so relative imports between the files keep working.
* `target`: where it lands, relative to the solution. Default as above; `<slug>` is made from the manifest's `id`
  (`@scope/name` becomes `scope-name`), or chosen with `ImportOptions::slug`.
* `core`: always copied. `modules`: the parts a host lets the user choose. A module should depend on the core only.
* `provided`: packages the solution supplies besides the ones an SPFx solution always does (`@microsoft/sp-*`, `react`,
  `react-dom`, `typescript`, `@types/*`).
* `notes`: plain text for the user after the import.

## The calls

```rust
inspect_import(&solution, &source, &manifest) -> Result<ImportState>      // reads, writes nothing, never fails for a missing peer
plan_import(&solution, &source, &manifest, ImportOptions) -> Result<ImportPlan>
apply_import(&plan, |progress| {}) -> Result<WriteOutcome>                // the same checks and rollback as apply_add
plan.undo_record("label")                                                 // before apply_import; undone with revert_edit
```

`ImportOptions { modules, slug, add }`: build it with `ImportOptions::new()` and set the fields (the struct is
`#[non_exhaustive]`). `modules` are the names chosen, in any letter case; the core is always imported. `add` is the
`AddOptions` of any add (limits, `package_manager`).

`ImportPlan { add: AddPlan, dependencies: Vec<DependencyOutcome>, notes: Vec<String> }`. `add` is the plan to show and apply:
its `files` (`New`, or `Merged` for `package.json` with `previous` for a diff), `kept`, `warnings` and `post` (an `install`
step, only when `package.json` changes). `has_clashes()` says whether some need clashes.

`ImportState { core, modules, dependencies }`: `core` and each of `modules` are a `ModuleState { name, title, state, present,
missing, differs }` (paths below the solution, never content) with `state` one of:

| `ModuleStatus` | Meaning | A form's switch |
|---|---|---|
| `Absent` | none of its files is there | off |
| `Present` | every file is there and the same (line endings do not count) | on, locked |
| `Modified` | every file is there and at least one differs | on, locked, "changed since the import" |
| `Partial` | some files are there, some missing | off; switching it on creates what is missing and keeps what differs |

The engine never deletes, so a switch that is on cannot be turned off by an import: removing a module is done by hand, or by
undoing the import while its files are untouched.

## What happens to each file

A file that is not in the solution is created. One that is there and the same is skipped. One that is there and **differs is
kept as it is**, listed in `AddPlan::kept` and in a warning. A file whose name looks like a secret (`.env`, `*.pem`, ...) is not
copied and a note says so. A second import with more modules works: the core that is already there is skipped. A target folder
that exists with the name in another letter case is refused.

## What happens to each need (`DependencyOutcome`)

For each of the package's `dependencies` and `peerDependencies`, against the solution's `dependencies` and `devDependencies`:

| Outcome | When | Effect |
|---|---|---|
| `Add { name, range }` | the solution does not have it | added to the solution's `dependencies`, after the existing ones, in the file's own format |
| `Skip { name, have, want, have_from }` | the solution has a version the range allows | nothing |
| `UpgradeAvailable { name, from, to }` | older than the range, same major version | **reported, not applied** (upgrading is not built) |
| `Clash { name, have, want, reason }` | another major version, or no version fits both, or the engine cannot tell | **never changed**; the files are still imported and a warning says what to settle by hand |
| `Untouched { name, category }` | the solution supplies it itself (`spfx`, `react`, `typescript`, `types`, `provided`) and has it | nothing |
| `MissingRequired { name, range, category }` | the solution supplies it itself and does not have it | **stops the import** |
| `MissingPeer { name, range }` | a peer the solution does not have | **stops the import** |

`plan_import` returns `ScaffoldError::Incompatible` naming every blocking need; `inspect_import` returns them as data.

* **The version the solution has** is read, in this order, from `node_modules/<name>/package.json` (`HaveFrom::NodeModules`),
  from `package-lock.json`, formats 2 and 3 (`HaveFrom::Lock`), and from the lowest version of its own declared range
  (`HaveFrom::Declared`, the least reliable: the lowest of `^4.2.0` is 4.2.0 whatever is installed). Yarn and pnpm locks are
  not read; pnpm's `node_modules` works as it is. A peer or a supplied package counts as present when it is installed, even if
  the solution does not declare it (`@microsoft/sp-http` comes with `sp-webpart-base`).
* **Ranges** follow npm (`^`, `~`, `||`, x-ranges, partial versions). **When the engine cannot decide it says so**: a
  pre-release, a path or URL, a tag such as `latest`, a hyphen range or anything it cannot read is a `Clash` whose reason
  starts with `could not tell`, never a guess. "Same major" means the major of the lowest version the package's range allows.

## What it warns about

* an existing file that differs (kept); a clash; an upgrade that is available and not applied;
* the warnings of any `package.json` change (scripts that run by themselves, as for an add);
* **an import the code makes that nothing declares**: the copied `.ts`/`.tsx`/`.js` files are scanned for `from "x"`,
  `import "x"`, `import("x")` and `require("x")`; a subpath maps to its package (`@pnp/sp/webs` is `@pnp/sp`); relative paths, Node
  built-ins and names the solution's `tsconfig.json` maps through `paths` are ignored; a package neither the package nor the
  solution declares is a warning.

## Not built yet

Upgrading a package the solution has (`UpgradeAvailable` is only reported); families of packages that must move together
(`@pnp/*`); projects that are not SPFx (`plan_import` takes an `SpfxSolution`); the authoring checks of a package (a module that
imports another, a root file that re-exports modules); a scan of file contents for secrets (only the names are checked); a warning
when the package asks for a `typescript` the solution does not satisfy; the lock-file warning after an upgrade; multi-select
questions on a form (`options` on a `list` prompt). The spec lives in the panel's repository
(`docs/service-packages-engine-spec.md`).
