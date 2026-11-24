# Writing your own template

This is the guide for someone who wants to make a template for the Forge scaffolding engine instead of using the ones Forge
provides. It says how to make one in two ways, what every part of a manifest means, and how to try it before using it.

A template you make or take from anywhere else is a **custom template**, and it is trusted by whoever uses it:
[`custom-templates.md`](custom-templates.md) says what that means and what the engine does and does not protect you from.
Read it before you share a template or use one you did not write.

## Two ways to make a template

**1. From a project you already have.** The quickest way: take a project that works, say which texts in it are "the
project's name" (and anything else that should change per project), and let the engine turn it into a template. It does not
guess which words are names; you tell it.

```text
forge-scaffold create ./my-app --id my-template --version 1.0.0 \
    --replace projectName=my-app --replace company=Acme --label projectName="Project name" \
    --title "My template" --description "A small starter." --tag custom --tag node \
    --out ./templates/my-template.1.0.0
```

Every occurrence of `my-app` in file contents and in file and folder names becomes the token `{__projectName__}`, and its
other casings become `{__projectNamePascal__}` (`MyApp`), `{__projectNameCamel__}` (`myApp`), `{__projectNameAllCaps__}`
(`MY_APP`). The report says how many times each text was replaced and in how many files, and what was left out and why:
`node_modules`, `dist`, `build`, `target`, `.git` and other folders a new project regenerates; `.env` files, keys and
certificates (they may hold secrets); lock files; and links. `.gitignore` is stored as `_gitignore` and written back under
its real name.

It **verifies itself**: the engine makes a project from the template with your original texts as the answers and compares it
with the project you gave it, file by file and byte for byte. A template that does not reproduce the project is refused, not
returned. From Rust the same thing is `create_template`; the program above is `forge-scaffold create`.

**What a list shows.** `--title`, `--description` and `--tag` (all optional) go into the manifest as `title`, `description` and
`tags`, which is what the panel's list shows for the template. They are checked by the same rules as everywhere else, and every
problem is reported with the field it is about: a title of at most 80 characters, a description of at most 300, no line breaks,
no space at either end, and at most 8 tags, each 1 to 30 lowercase letters, digits or `-`, none twice. With none given the
manifest has none, and the list shows the id.

**From Rust, and what the report holds.** `create_template(project, &CreateOptions)` returns a `TemplateDraft` in memory, and
`draft.report()` is the plain value a screen shows: `files` (how many the template holds), `skipped` (each by `path` and
`reason`), `counts` (each replacement by `key`, `text`, `occurrences` and `files`) and `notes`. Those fields are the contract and
stay as they are; `report.to_json()` is the same for a program that is not written in Rust. **The report never holds the
contents of a file**: a skipped `.env` is a path and a reason, and neither the report, the errors nor `{:?}` of the draft print
file contents, so they are safe to show and to log. (A text you name for a replacement is your own input and is shown as given.)

**Putting it where it is used.** `store.add_local(&draft)` writes the template into the templates folder as the user's own and
returns its folder. It is named `<id>.<version>`, the way the downloaded ones are, so a host never builds the path, and the
template is listed at once with `local` set. It never overwrites: it is refused, with the reason and the field, if the id is not
lowercase letters, digits and `-`, if a template with this id and version is already there (downloaded or not, under any folder
name), if the folder name is taken, or while an update runs. It writes beside the destination and renames into place, so the list
never sees half a folder, and it leaves nothing behind when a write fails. `draft.write_to(folder)` is still there for a host that
chooses the folder itself (it also refuses to overwrite).

Limits worth knowing: a text must be at least two characters (a shorter one would match inside other words), a text that is not
in the project is an error, and a project that already contains text shaped like a token (`{__name__}`) is refused as a whole, because
the engine has no way to write one literally and leaving the file out would give a template that does not reproduce the project;
the error names up to five of the files by path and never shows what is in them. If two texts share a form (`hello-world` and `Hello World` are both
`HelloWorld` in Pascal case), the one named first holds it, and the report says so.

**2. By hand.** A template is a folder with a `manifest.json` and the files it copies. Writing one by hand is the way when the
template has choices (a language, a framework), lists (one page per entry), or edits to shared files. The next sections are the
reference, and there is a worked example.

## Where a template goes

Put the folder in the engine's **templates folder** (the host passes its path; otherwise the `FORGE_TEMPLATES` environment
variable, otherwise `<data dir>/mentenaz/templates`). A folder there that is not one the engine downloaded is your own: it is
never changed or removed by an update, and it wins over a download with the same generator id and version. A host shows it as
the user's own template.

## The folder

```text
my-template.1.0.0/
  manifest.json
  base/                       folders the manifest names; their contents are what gets copied
    package.json              text files may hold tokens: {__projectName__}
    src/{__projectNamePascal__}.ts     so may file and folder names
    _gitignore                written as .gitignore
```

The folder name does not matter; the manifest's `id` and `version` do. Only a folder that holds a `manifest.json` is a template.

## Tokens

A **token** is `{__name__}`: a name of letters and digits between `{__` and `__}`. It is replaced in file **contents** and in
file and **folder names**, and in the manifest's own strings where it says tokens may be used. A token that nothing defines is
an error (a typo is caught when the template is checked, not left in a file). Text that is not token-shaped is left alone.

Where tokens come from:

| Source | Tokens | Example for the answer `my-app` |
|---|---|---|
| a **text** prompt `key` | `{__key__}` and four derived forms | `{__key__}` `my-app`, `{__keyPascal__}` `MyApp`, `{__keyCamel__}` `myApp`, `{__keyKebab__}` `my-app`, `{__keyAllCaps__}` `MY_APP` |
| a **choice** prompt | `{__key__}`: the chosen option | `ts` |
| a text prompt with `"sensitive": true` | `{__key__}` only: the text as typed | (no derived forms, and it is never logged) |
| `guids` | one generated GUID per run, `{__name__}` | `"guids": ["solutionId"]` |
| `secrets` | one generated secret per run: letters and digits, with an upper case letter, a lower case letter and a digit | `"secrets": ["sessionSecret"]` |
| a `repeat` (see below) | the current list item, with the same five forms, and its own `guids` | `"as": "page"` gives `{__page__}`, `{__pagePascal__}`, ... |

An answer is put in exactly as typed. It is never read as a token again, so an answer cannot pull in another value.

## The manifest

`manifest.json` may start with `"$schema"` (the `Manifest.v1.json` schema in the templates repo) so an editor checks it as you
type. Every field the engine does not know is an error.

| Field | What it is |
|---|---|
| `id` | the generator's id, `my-template` |
| `version` | three numbers, `1.0.0`. For a template made from a tool, that tool's version |
| `prompts` | the questions (below) |
| `folderFrom` | the key of the **text** prompt whose answer is the name of the new folder. It must be one plain name: no `/`, no `..` |
| `include` | folders copied once |
| `repeat` | folders copied once per item of a list prompt |
| `parts` | a file assembled from fragments, in order |
| `merge` | edits to JSON files after everything is rendered |
| `post` | steps for the host to run afterwards: `install` and `composeUp` only |
| `guids`, `secrets` | generated values, as tokens |
| `packageManagers` | `{ "default": "npm", "tested": ["npm", "pnpm"] }`: which the template was tried with. A host warns about an untried one |
| `requires` | `{ "node": ">=20.0.0" }`: the Node versions the generated project needs (an npm-style range). A host can warn before installing |
| `nestedMarker` | a file whose presence in the location (or a parent) means "this is inside an existing project": a warning, not an error |
| `add` | how the template is added to an existing SPFx solution. Only for the SPFx templates |

### prompts

Each prompt is one question, with a `key` (the token name; letters and digits, starting with a letter), a `type`, and a `label`
the form shows.

| `type` | Answer | Also |
|---|---|---|
| `text` | a string | `default`, `sensitive` (hidden input, plain token only), `rule` |
| `bool` | true or false | `default` (false if left out) |
| `list` | a list of strings (empty is allowed unless a rule forbids it) | `rule` |
| `choice` | one of `options` | `options` (required, only here), `default` |

A prompt can have a `when`, the same condition the includes use, and is then asked only when it holds: `{ "key": "dbKind",
"type": "choice", "options": ["postgres", "mysql"], "when": { "key": "database", "equals": true } }`. The condition may test only
a `choice` or `bool` prompt that comes **earlier** in the list (so there are no cycles), and `all` combines several. A prompt that
is not asked has no answer, is not checked, and defines no token, so a file that always uses `{__dbKind__}` fails in the cases
where it is not asked; put such a file in an `include` with the same `when`. An answer given for a prompt that is not asked is
ignored. The prompt named by `folderFrom` must always be asked. A host calls `Manifest::visible_prompts(&answers)` while the user
fills in the form to know which questions to show; `forge-scaffold form --answers` returns the same list as `visible`.
`check_template` renders the cases behind a conditional prompt too (the options of `dbKind` are tried when `database` is true).

`rule` checks the answer and reports every problem at once, one message per field:

| `rule` | On | Accepts |
|---|---|---|
| `solution-name` | text | lowercase letters, digits and `-`, starting with a letter, at most 50 characters (a name that is also a folder and a package name) |
| `package-name` | text | an npm package name: lowercase, digits and `- . _ ~`, not starting with `.` or `_`, optionally `@scope/name`, at most 214 characters |
| `webpart-names` | list | letters and digits, starting with a letter, no `WebPart` at the end, no repeats ignoring case (for SPFx web parts) |

A text answer is trimmed before it is used. `folderFrom` must not be scoped (`@scope/name`), because a folder is one name.

### include, repeat, parts

```json
{ "folder": "base", "to": "app", "rename": { "_gitignore": ".gitignore" }, "when": { "key": "language", "equals": "ts" } }
```

`include` copies everything under `folder`; `to` puts it under a subfolder of the new project; `rename` changes a file's own
name wherever it is in the folder (a template repository cannot hold a real `.gitignore` for the project it makes, so the
template keeps `_gitignore`); `when` makes it conditional. `repeat` is the same with `over` (a list prompt) and `as` (the
token name for the current item), once per item, and it may have its own `guids` (a fresh GUID per item).

`parts` builds one file from several fragments: `{ "target": "docker-compose.yml", "source": "fragments/db.yml", "when": ... }`
entries with the same `target` are joined in the order written.

### when

A condition, used on `include`, `repeat`, `parts`, `merge` and `post`:

```json
{ "key": "language", "equals": "ts" }
{ "key": "db", "in": ["postgres", "mysql"] }
{ "all": [ { "key": "language", "equals": "ts" }, { "key": "install", "equals": true } ] }
```

`key` names a `bool` or `choice` prompt. A key that does not exist never matches (and the manifest check reports it).

### merge

Edits a JSON file after the files are rendered. `strategy` is `json-set` (set a dotted `path`, creating the objects on the way)
or `json-append` (append to the array at `path`, creating it; an error if the path holds something that is not an array). The value is `value` (a JSON value whose strings may hold
tokens) or `valueFile` (a template file with a JSON fragment, rendered first and then parsed, so `"port": {__port__}` stays a
number). `per` applies the merge once per item of a list prompt, which must be a `repeat`'s `over`. `file` and `path` may hold
tokens, and `when` makes it conditional.

### post

`{ "action": "install" }` or `{ "action": "composeUp" }`, with an optional `cwd` (a folder inside the new project, never outside
it) and `when`. The engine **runs nothing**: it hands these to the host as data, and the host decides whether and how to run
them. Nothing else can be named; a field such as `command` is an error.

## What a template cannot do

These are refused, and the message says which file:

- a path that goes outside the new folder (`..`, an absolute path, a drive letter, `\\server\share`), in the template or built
  from an answer;
- a name Windows keeps for devices (`nul`, `con`, `com1`, ...), a name ending in a dot or a space, characters Windows rejects
  (`< > : " | ? *`), and a `.git` folder (where a hook would run code);
- a link (a symbolic link or a junction) in the template folder;
- more than 20,000 files, a file over 64 MiB, or more than 512 MiB in all (a host may set lower limits);
- a field the engine does not know, or a command in `post`.

## A worked example

A site with a choice (TypeScript or JavaScript), a list (one page per entry), a generated secret, a JSON file that gains an
entry per page, and an optional install step. Its files are in `tests/fixtures/authoring-example/`, and a test checks that this
manifest is exactly the one the engine accepts.

```text
pages-app/
  manifest.json
  base/package.json        {"name": "{__projectName__}", ...}
  base/_gitignore
  base/site.json           {"title": "{__projectNamePascal__}", "pages": []}
  base/.env.example        SESSION_SECRET={__sessionSecret__}
  ts/tsconfig.json         copied only for TypeScript
  js/jsconfig.json         copied only for JavaScript
  page/pages/{__pageKebab__}.html     copied once per page
```

```json
{
  "id": "pages-app",
  "version": "1.0.0",
  "folderFrom": "projectName",
  "prompts": [
    { "key": "projectName", "type": "text", "label": "Project name", "rule": "package-name" },
    { "key": "language", "type": "choice", "label": "Language", "options": ["ts", "js"], "default": "ts" },
    { "key": "pages", "type": "list", "label": "Pages" },
    { "key": "install", "type": "bool", "label": "Run install afterwards", "default": false }
  ],
  "secrets": ["sessionSecret"],
  "include": [
    { "folder": "base", "rename": { "_gitignore": ".gitignore" } },
    { "folder": "ts", "when": { "key": "language", "equals": "ts" } },
    { "folder": "js", "when": { "key": "language", "equals": "js" } }
  ],
  "repeat": [
    { "over": "pages", "as": "page", "folder": "page" }
  ],
  "merge": [
    {
      "file": "site.json", "strategy": "json-append", "path": "pages", "per": "pages",
      "value": { "name": "{__page__}", "file": "pages/{__pageKebab__}.html" }
    }
  ],
  "post": [
    { "action": "install", "when": { "key": "install", "equals": true } }
  ],
  "packageManagers": { "default": "npm", "tested": ["npm", "pnpm"] }
}
```

With the answers `projectName: my-site`, `language: js`, `pages: ["About Us", "Contact"]` and `install: true` it writes:

```text
.env.example   .gitignore   jsconfig.json   package.json
pages/about-us.html   pages/contact.html   site.json
```

`site.json` has one entry per page (`{ "name": "About Us", "file": "pages/about-us.html" }`), `pages/about-us.html` reads
`<h1>About Us</h1>`, `.env.example` holds a generated secret, and the plan hands the host an `install` step for `npm`.

## Trying a template

**Check it.** `forge-scaffold check ./my-template.1.0.0` (from Rust, `check_template`) loads the manifest, validates it, and
renders the template with sample answers, once with the defaults and once for every other value of every yes/no and choice
prompt, so a branch behind a `when` is rendered too. It writes nothing. The report has:

- **problems**: the manifest does not load, or a variant does not render (with the error, which names the file), or a limit
  was hit. A template with a problem should not be used.
- **notes**: a prompt nothing uses, a file the manifest never copies, and too many variants to render all of them.
- **per variant**: how many files and bytes it makes, the steps it hands the host, and the engine's warnings, which are about
  what the template does, not whether it works: scripts that run when the project is installed, dependencies that come from a
  URL or a git repository, and files tools run by themselves (`.husky/`, `.vscode/tasks.json`, `.npmrc`, ...).

Pass `--answers answers.json` to fix some answers. The check is structural: it does not read the code the template writes.

**Then make a project.** `forge-scaffold plan --generator my-template --location ./out --answers answers.json` says what would be
written and writes nothing; `apply` writes it. `forge-scaffold form --generator my-template` prints the questions a host would ask.
Every command prints JSON, one object per line, and the last line is the result.

## Sharing a template

Give someone the folder, or build archives and an `index.json` for your own server: `docs/publishing.md` describes the release
builder (`cargo run --example release`, `build_release`) and the index a host downloads from. Anyone who uses a template you
share is trusting you, and you are asking them to; say what it does, and keep scripts and URL dependencies out of it unless
they are needed.

## Common mistakes

- **"unknown token"**: a `{__name__}` that no prompt, `guids`, `secrets` or `repeat` defines. Spelling and case matter
  (`{__projectName__}`, not `{__projectname__}`).
- **A literal `{__x__}` in a file**: not possible; the engine has no escape. Change the text.
- **A `merge` that finds no file**: the file must be one the manifest copies (`merge` edits the rendered result, not the
  template folder).
- **A prompt that is never used**: reported as a note. Use it as a token or in a `when`, or take it out.
- **`folderFrom` pointing at a `list` or `bool`**: it must be a text prompt.
