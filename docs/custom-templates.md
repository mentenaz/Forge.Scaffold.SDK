# Custom templates: where they go, how they work, and who is responsible

> **Draft for the Forge documentation.** The "Trust and responsibility" section states what the project owner asked to be
> stated. It is not legal advice: have it read by someone qualified before it is published as terms.

Anyone can use their own templates with the engine instead of (or next to) the ones Forge provides. This page says where
they go, what a template is, how the engine limits harm, and where its responsibility ends.

## Where custom templates go

The engine reads generators from a templates folder (the host passes its path; otherwise `FORGE_TEMPLATES`, otherwise
`<data dir>/mentenaz/templates`). A folder in it that is **not** listed in `installed.json` is the user's own:

- it is never changed, replaced or removed by an update or a download;
- when it has the same generator id and version as a downloaded one, it wins;
- it shows in the version list as the user's own folder (`local`), so a host can label it.

A generator folder is a `manifest.json` and the template files it names. `Manifest.v1.json` (a JSON schema, in the templates
repo) lets an editor check a manifest as it is typed, and loading it reports every problem together, one message per field.
`cargo run --example scaffold -- <generator folder> <location> <name> ...` tries a folder from the command line.

## What a template is

```text
my-template/
  manifest.json
  app/                     a folder the manifest copies
    package.json           text files may use tokens: {__projectName__}, {__projectNamePascal__}, ...
    src/{__projectName__}.ts
    _gitignore             written as .gitignore (see "rename")
```

```json
{
  "id": "my-template", "version": "1.0.0", "folderFrom": "projectName",
  "prompts": [ { "key": "projectName", "type": "text", "label": "Project name", "rule": "package-name" } ],
  "include": [ { "folder": "app", "rename": { "_gitignore": ".gitignore" } } ],
  "post": [ { "action": "install" } ]
}
```

What a manifest can express: prompts (text, yes/no, list, choice, with defaults and name rules), tokens in paths and in text
with derived casings, folders copied once (`include`) or once per list item (`repeat`), files assembled from fragments
(`parts`), edits to JSON files (`merge`), conditions (`when`), file renames, an engine version it needs (`requires.node`), and
a fixed set of follow-up steps for the host (`install`, `composeUp`). That is all it can express: a manifest holds no
commands and no code, and unknown fields are an error.

**Tools for making one:** `create_template` (or `forge-scaffold create`) turns a project you already have into a template, and
verifies by scaffolding it back that the template reproduces the project. `check_template` (or `forge-scaffold check`) loads a
template folder, validates the manifest and renders it with sample answers in every branch, without writing anything.
[`authoring-templates.md`](authoring-templates.md) is the guide: both tools, every manifest field, and a worked example.

## Trust and responsibility

**A custom template is trusted by the person who uses it.** A template is custom when it did not come from the Forge
templates server: one you wrote, one a colleague or a stranger gave you, or one you downloaded from anywhere else.

- By creating, importing or using a custom template you accept that **you** are responsible for what it does. The engine
  writes the files a template describes into your new project and hands the host the follow-up steps it names. It cannot
  tell what a template is *for*.
- **Mentenaz Forge, its authors and its contributors are not responsible for any loss or damage that comes from creating,
  importing or using a custom template**, including from code a template contains or fetches, to the extent the law allows.
  The licence of the engine (`MIT OR Apache-2.0`) already says it comes without warranty.
- This also holds for templates that Forge itself publishes from other projects (the SharePoint Framework and Vite ones):
  they are third-party code with their licences, converted for the engine, and are provided as they are.
- A scanner that checks templates for inconsistencies is being built as a **separate engine**, and will come later. It will
  help; it will not make a template safe, and its absence today means nothing is checked for you beyond what is listed below.
  It will be a dependency of Forge, used to validate what a template writes before an install. Until it is finished, tested and
  confirmed, the places it will plug in are only marked in the source with `TODO(forge-analysis)` (nothing calls it).

### What the engine does to limit harm

Each of these is covered by a test (`tests/hostile_templates.rs`, and unit tests in `stage.rs`, `archive.rs` and `pack.rs`).

- **Nothing is written outside the new folder.** Every path, including paths made from the user's answers, is checked after
  the tokens are filled in: no `..`, no absolute paths, no drive letters or `\\server\share`, no characters Windows rejects,
  no names ending in a dot or a space, no Windows device names (`nul`, `con`, `com1`, ...), and no `.git` folder (where a
  hook would run code). A bad answer fails the plan, and planning writes nothing.
- **A new project goes in a folder that does not exist yet**, and is removed again if a write fails halfway. Adding to an
  existing solution never deletes a file and never replaces one, except by the add-only merges of its JSON files (which keep
  what is there), checks that nothing changed since it was planned, and puts everything back if a write fails.
- **Links are refused**, both in a template folder (symbolic links and Windows junctions) and in an archive (symbolic links,
  hard links, devices, fifos). An add to an existing solution also refuses to write through a link inside the solution, both
  when it is planned and again when it is applied, so a file cannot end up somewhere else on the machine. (Checked with
  Windows junctions; Unix symbolic links are covered by the same code and not yet run on Linux.)
- **Size limits.** A template folder may make at most 20,000 files, 64 MiB per file and 512 MiB in all; a file that is too big
  is refused before it is read. A host can set lower (or higher) limits for its own use (`Limits`, in `PlanOptions` and
  `AddOptions`, and `DirSource::with_max_file_bytes`). A downloaded archive is held to 5,000 files, 20 MiB per file and 50 MiB
  in all, so a small archive that expands enormously is refused.
- **Answers are text, not code.** An answer is put in as typed and is never read as a token again, so it cannot pull in
  another value (such as a generated secret).
- **A manifest holds no commands.** The follow-up steps are a fixed list, `cwd` cannot leave the project, and a field the
  engine does not know is an error.
- **Downloads are verified** against the sha256 in the index before they are unpacked, and unpacked into a scratch folder.
- **It tells the host what installing would run.** The engine runs nothing, but when the new project's `package.json` has
  scripts the package manager runs by itself (`preinstall`, `install`, `postinstall`, `prepare`), or depends on something
  that is not a version from the registry (a URL, a git repository, a local path, `user/repo`), the plan carries a warning
  (`Warning::InstallScripts`, `Warning::UrlDependencies`). When an add merges into an existing `package.json`, only what the
  add brings in is reported, not what the solution already had. A third warning, `Warning::AutoRunFiles`, lists files that
  tools run or obey by themselves: `.husky/`, `.vscode/tasks.json`, a development container, `.envrc`, and package manager
  settings or plugins (`.npmrc`, `.yarnrc*`, `.pnpmfile.*`, `.yarn/plugins/`, `.yarn/releases/`). None of the templates Forge
  ships triggers any of these warnings.

### What it cannot do

- **Judge intent.** A template can write a project that is broken, misleading, or harmful when it is run. The files are the
  user's to read.
- **Stop an install from running code.** `npm install` runs a project's install scripts and its dependencies' scripts, and
  fetches whatever the dependencies name. The engine does not run npm: the host does. The warnings above are the engine's
  part; the rest is the host's and the user's.
- **Vouch for who made a template**, or for a downloaded index beyond its hash.
- **Find malicious or buggy code inside the files it writes.** That is what the planned scanner is for.

### Before using a template you did not write

1. Open `manifest.json`: what does it copy, edit and ask the host to run?
2. Open the `package.json` files: the scripts, and every dependency, especially any that is a URL or a repository.
3. Look for files that run on their own: `.husky/`, `.vscode/tasks.json`, shell scripts, CI files. The engine lists the first
   kinds in a warning; shell scripts and CI files it does not.
4. Run `forge-scaffold check <folder>` (or `check_template`) and read what it reports: problems, notes, and the engine's
   warnings for every branch of the template.
5. Install with scripts turned off the first time (for npm, `--ignore-scripts`), then decide.

### For hosts (the Forge panel and other apps)

The host runs the commands, so the host is where the user's consent belongs.

- Label a template from the user's own folder as custom (`ResolvedTemplate.local`), and show the warnings of the plan
  **before** running an `install` step, with the option to cancel or to install without scripts.
- Show the plan (the files, and which existing files would be edited) before writing.
- Never run a command a template names. The engine does not return one: only the fixed steps.
