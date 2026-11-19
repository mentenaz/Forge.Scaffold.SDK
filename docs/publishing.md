# Publishing templates

Source lives on GitHub. The built templates (archives and `index.json`) are served from
`https://downloads.mentenaz-server.com/forge-scaffold/`, next to the other downloads.

## Build

```powershell
cargo run --example release -- <out dir> https://downloads.mentenaz-server.com/forge-scaffold <generator folder>...
```

Pass every generator folder to publish (for example `templates/spfxv.*` in the templates repo). Templates of the official SharePoint/spfx CLI are added with `--upstream <spfx repo> <template>` (repeat per template) and are converted to a generator folder named `<template>.<spfx version>`. `--overlay <folder>` (a folder with an `overlay.json`, such as `layers/` in the templates repo) replaces their welcome images and Teams icons with your own. If `<out dir>/index.json`
already exists it is read first: an archive that is byte-identical keeps its `revision`, a changed one gets the next
number. Put the previous `index.json` in the out dir (download it from the server) before building.

Archives are deterministic (sorted entries, no times or owners, fixed gzip header) and newlines in text files are
normalized to `\n`, so a Windows and a Linux checkout give the same bytes. A new version of the crate's compression library
can change the bytes of an unchanged template; that only bumps its revision.

## Upload

Use SFTP (or rsync over SSH), not plain FTP, with a dedicated deploy user limited to the `forge-scaffold` folder.

1. Upload the `.tar.gz` files first.
2. Upload `index.json` last, so clients never see an index that points at a file that is not there yet.

## Server

- Serve over `https` only (the engine rejects any other URL) and keep the certificate renewing.
- Send `ETag` and answer `If-None-Match` with `304`. Static files on nginx or Apache do this by default.
- Give `index.json` a short cache lifetime, and the archives a long one (their names include the version).
- The app only needs the index URL. Every archive has its own full URL inside the index, so archives can move later by
  editing the index.

## Licence notices

- **Official SPFx templates** (`--upstream`): they are MIT licensed, Copyright (c) Microsoft Corporation, and the MIT
  licence requires its notice to stay with copies. Every archive of an upstream template therefore carries a `NOTICE.txt`
  at its top: where the template comes from, what was changed (the tags became engine tokens, a `manifest.json` was added),
  a line saying how many images the overlay replaced (those are not Microsoft's and not covered by the MIT text), and the
  `LICENSE` file of the SPFx repo verbatim. The notice has no dates, so archives stay deterministic. If the SPFx checkout has
  no `LICENSE` file, the release is refused.
- The notice sits in the installed template folder, so a panel can show it ("About this template"). The projects generated
  from a template do not get one: they are the developer's own, and the official CLI adds none either.
- **Our own `yo`-derived generators** are a different matter: they are derived from `@microsoft/generator-sharepoint`
  output, not from the MIT repo, and their licence is the open question in SharePoint/sp-dev-docs discussion 11052. A
  generator folder can carry its own `NOTICE.txt`; whatever is in the folder goes into the archive.

## Before anything is public

Check that `NOTICE.txt` is in every upstream archive you are about to upload, and record the provenance of the `yo`-derived
generators (the templates repo's `README.md` does that).
