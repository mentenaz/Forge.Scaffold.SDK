# The public API and how it is versioned

This is for whoever depends on the crate (the Forge panel is the first), and for whoever changes it.

## What is the API

**What `src/lib.rs` re-exports at the crate root, without `#[doc(hidden)]`, is the supported API.** That is all a host
should use. The modules are private on purpose, so there is one path to every name (`forge_scaffold::TemplateStore`,
`forge_scaffold::plan`, ...) and nothing else can be depended on by accident.

`tests/public_api.rs` imports every one of those names. Removing or renaming one is a compile error there, so it is always a
deliberate act: the test is changed in the same commit and `CHANGELOG.md` says so.

What is **not** the API:

- **The release pipeline** (`build_release`, `build_archive`, `pack_dir`, the SPFx and create-vite readers, the overlay). It is
  tooling for the templates repository and for `examples/release`. Its modules are `pub` and its re-exports are
  `#[doc(hidden)]`, so the examples can use them and the documentation does not offer them. It can change in any release.
- **Anything inside a module that is not re-exported**: limits, casing helpers, version helpers, constants. They are
  `pub(crate)`.
- **The examples and the `forge-scaffold` source.** They show how to use the API; they are not part of it.

## Versions

The crate follows semantic versioning, read the way 0.x asks for:

| change | version |
|---|---|
| a supported name is removed or renamed, a signature changes, a field of a struct a host constructs is added or removed, a variant is removed | **minor** (0.2 to 0.3) while the crate is 0.x |
| a function, method, type or variant is added; a bug is fixed; the docs change | **patch** (0.2.0 to 0.2.1) |

The enums a host receives (`Warning`, `ScaffoldError`, `PostAction`, `PromptKind`, ...) are `#[non_exhaustive]`, so a new
variant is a patch change and a host must match them with a fallback arm. A host builds the structs it passes in through their
constructors (`PlanOptions::new`, `AddOptions::new`, `CheckOptions::default()`), not with a struct literal, so a new field does
not break it.

Before a supported name is removed, it is marked `#[deprecated(note = "...")]` for one minor version when that is possible, and
the changelog says what replaces it.

**1.0** is a decision for the owner, not a number the crate reaches: it means the panel and the engine have settled and the
supported API will only break on a major version. `tests/public_api.rs` fails if the version stops starting with `0.`, so that
decision is made on purpose.

## Documentation is part of the API

Every supported item has a doc comment: the crate denies `missing_docs`, so a name that is re-exported at the root without one
does not compile. Examples in the docs are doctests, so `cargo test` keeps them honest. Items in the hidden pipeline modules
are exempt.

## The JSON formats have their own numbers

The crate reads and writes JSON that outlives one build, and each kind carries its own number, separate from the crate version:

| format | where the number is | now | changes when |
|---|---|---|---|
| the template index | `schema` in `index.json` (`INDEX_SCHEMA`, `MIN_INDEX_SCHEMA`) | 2 (1 is still read) | a field is removed, renamed or changes meaning |
| a pack | `schema` (`PACK_SCHEMA`) | 1 | the same |
| an undo record | `format` (`UNDO_FORMAT`) | 1 | the same |
| the `forge-scaffold` program's results | `protocol` in every result | 1 | the same |

**Adding a field or an optional value never changes a number.** A change that does is also a minor version of the crate, and
the changelog says what a reader of the old format loses. The crate reads the older formats it lists as readable (the index
reads 1 and 2) and refuses newer ones with a clear message, never a guess.

## Releasing a version

1. Update `CHANGELOG.md`: what was added, changed (say **breaking** where it is), fixed.
2. Set `version` in `Cargo.toml` (and let `Cargo.lock` follow).
3. `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`.
4. Commit, push, and wait for CI on Ubuntu, Windows and macOS.
5. Tag the commit `vX.Y.Z` and push the tag. **The panel pins the tag**:
   `forge-scaffold = { git = "https://github.com/mentenaz/Forge.Scaffold.SDK", tag = "v0.2.0" }`.
