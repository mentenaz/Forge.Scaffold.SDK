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

## Publishing with GitHub Actions (draft)

The workflow is `.github/workflows/publish.yml` in the templates repo. It only runs when started by hand (Actions, Publish
templates, Run workflow), and it has two jobs:

1. **build** (no secrets). Checks out the templates repo, the engine and the SPFx fork, then `tools/publish_build.sh`:
   downloads the index published last time (so unchanged archives keep their revision; a 404 means first publish, anything
   else stops), builds every archive and the index, and checks them before anything can be uploaded: sha256, size and URL
   against the index, no `.git` or `node_modules`, `manifest.json` present, and `NOTICE.txt` with Microsoft's copyright line in
   every official template. The result is a table in the run summary (new, unchanged, CHANGED) and a downloadable artifact.
2. **publish** (only when the `publish` box is ticked; environment `production`; the only job that sees the SFTP secrets).
   `tools/publish_upload.sh` sends every archive and then `index.json` last, each under a temporary name that is renamed into
   place, so a client never sees an index that points at a missing file and never a half-sent file. Nothing on the server is
   deleted. `tools/publish_verify.py` then downloads the index and every archive from the public URL and compares them with
   what was built.

### One-time setup

1. **Push the templates repo to GitHub.** It has no remote yet. The engine is at `github.com/mentenaz/Forge.Scaffold.SDK`.
2. **The server.** A deploy user that can only use SFTP (no shell), confined to the downloads folder (for example OpenSSH
   `ChrootDirectory` with `ForceCommand internal-sftp`), with write access to `forge-scaffold/` and nothing else. The server
   should be OpenSSH: replacing an existing file by rename uses its `posix-rename` support. Otherwise the step fails at that
   rename and the old file stays.
3. **A key for that user.** `ssh-keygen -t ed25519 -C "forge-scaffold publish" -f deploy_key`. Put `deploy_key.pub` in the
   user's `authorized_keys` (a leading `restrict` is a good idea), and keep `deploy_key` for the secret below.
4. **The server's host key, pinned.** Run `ssh-keyscan -t ed25519 downloads.mentenaz-server.com` on your own machine, and
   check the fingerprint against the server's own `/etc/ssh/ssh_host_ed25519_key.pub` (`ssh-keygen -lf`) before you trust it.
   The workflow refuses to connect to a host whose key is not in this secret: there is no trust on first use.
5. **GitHub settings.**
   - Variable `TEMPLATES_BASE_URL`: `https://downloads.mentenaz-server.com/forge-scaffold`.
     Optional variables `SDK_REPOSITORY` and `SPFX_REPOSITORY` if the repos are not `mentenaz/Forge.Scaffold.SDK` and
     `mentenaz/spfx`.
   - Environment `production` with a required reviewer, holding the secrets `SFTP_HOST`, `SFTP_USER`, `SFTP_PATH`
     (for example `/var/www/downloads/forge-scaffold`, letters, digits and `. _ - /` only), `SFTP_KEY` (the private key),
     `SFTP_KNOWN_HOSTS` (the line from step 4) and optionally `SFTP_PORT`.
   - If the engine or the SPFx fork are private repos: a secret `CHECKOUT_TOKEN`, a fine-grained token with read access to
     their contents.
6. **A first run with `publish` off.** Read the table in the run summary and the artifact. Then run it with `publish` on and
   approve it.

### Safety

- The workflow has `contents: read` permissions only, runs one publish at a time and never cancels one halfway.
- The build job cannot see the SFTP secrets. Every action is pinned to a commit (keep them current with Dependabot).
- The scripts accept only plain characters in the paths and names they write into the SFTP batch.
- Let the workflow be the one that publishes: the revision of each archive depends on its bytes. A Windows build and the
  workflow's Linux build were compared once (19 November 2026, publish off): all 25 archives and `index.json` were byte for
  byte identical, from the same inputs (the SPFx fork at `c2fd2f5`, the engine at `a1c9e8a`, the templates repo at `845beaf`).
  The archives are deterministic and newlines are normalized, so that should hold, but it is one comparison.

### What was tested, and what was not

Tested on a developer machine:

- The workflow file: structure, manual trigger only, all eight actions pinned to commits, the SFTP secrets referenced only in
  the publish job.
- `publish_build.sh` against the real checkouts: 25 archives (20 official, 5 `yo`-derived) built and checked in about 40
  seconds; first publish; a 404 for the previous index; revisions kept from a previous index (identical index); entries
  dropped from a release reported; refusal of a non-empty output folder and of an SPFx checkout without `LICENSE`.
- `publish_verify.py` against a local web server: passes on a good copy, catches a tampered archive, refuses plain http for a
  real host.
- `publish_upload.sh` with the real OpenSSH `sftp` client against a stand-in SFTP server (a throwaway script, since deleted):
  archives before `index.json`, renamed into place, files replaced in place on a second upload, no `.part` files left, a wrong
  pinned host key and an unknown key both refused with nothing sent, and a server that refuses to write the index leaves the
  archives uploaded, the old index untouched and the step failed.

The build job on GitHub, run by hand with publish off (19 November 2026, 58 seconds): every step passed and the upload job was
skipped. Against the real domain, the previous-index request returned 404, so it took the first-publish path. It built 25
archives, and the downloaded artifact is byte for byte the build made on this machine.

Not tested:

- The upload job on GitHub (the SFTP upload and the check of what the server serves). It has never run.
- Your real server: its SFTP configuration, `posix-rename`, permissions, and the ETag and cache headers (the verify step warns
  when there is no ETag).

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
