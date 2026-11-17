# Publishing templates

Source lives on GitHub. The built templates (archives and `index.json`) are served from
`https://downloads.mentenaz-server.com/forge-scaffold/`, next to the other downloads.

## Build

```powershell
cargo run --example release -- <out dir> https://downloads.mentenaz-server.com/forge-scaffold <generator folder>...
```

Pass every generator folder to publish (for example `templates/spfxv.*` in the templates repo). If `<out dir>/index.json`
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

## Before anything is public

The templates are derived from `@microsoft/generator-sharepoint` output. Check that package's license and record the
provenance (see `README.md`) before the first upload.
