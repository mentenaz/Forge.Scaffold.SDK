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

The host (AfriHost) offers FTP over TLS (FTPS, port 21) and no SFTP, so the workflow uses FTPS, the same way the Forge Super
repo's `build_installers.yml` publishes its apt repository. Never plain FTP: the password would cross the internet unencrypted.
If the account can be limited to the `forge-scaffold` folder, do that.

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
2. **publish** (only when the `publish` box is ticked; environment `production`; the only job that sees the FTP secrets).
   Two runs of `SamKirkland/FTP-Deploy-Action` (pinned to a commit) send every archive and then `index.json` last, so a client
   never sees an index that points at a missing file. Nothing on the server is deleted. The action keeps a state file per step
   in the folder to send only what changed. It does not rename into place, so a client could in theory fetch a file
   while it is being replaced; the archives are small. `tools/publish_verify.py` then downloads the index and every archive
   from the public URL and compares them with what was built.

### One-time setup

1. **Push the templates repo to GitHub.** It has no remote yet. The engine is at `github.com/mentenaz/Forge.Scaffold.SDK`.
2. **The server.** A cPanel FTP account `forgetemplates@downloads.mentenaz-server.com` whose directory is the `forge-scaffold`
   folder of the downloads site (served at `https://downloads.mentenaz-server.com/forge-scaffold`), so it cannot touch the
   apt repository next to it, which has its own account. Its FTP root is that folder, so the workflow uploads to `/`.
   (cPanel cannot change an account's directory: it must be deleted and created again to move it.)
3. **GitHub settings.**
   - Variable `TEMPLATES_BASE_URL`: `https://downloads.mentenaz-server.com/forge-scaffold`.
     Optional variables `SDK_REPOSITORY` and `SPFX_REPOSITORY` if the repos are not `mentenaz/Forge.Scaffold.SDK` and
     `mentenaz/spfx`.
   - Secrets `SFTP_HOST` (`downloads.mentenaz-server.com`), `SFTP_USER` (`forgetemplates@downloads.mentenaz-server.com`) and
     `SFTP_PASSWORD` (the names say SFTP but they are the FTPS login). `SFTP_PATH` is not used: the account's root is the
     folder.
   - If the engine or the SPFx fork are private repos: a secret `CHECKOUT_TOKEN`, a fine-grained token with read access to
     their contents.
4. **A first run with `publish` off.** Read the table in the run summary and the artifact. Then run it with `publish` on and
   approve it.

### Safety

- The workflow has `contents: read` permissions only, runs one publish at a time and never cancels one halfway.
- The build job cannot see the FTP secrets. Every action is pinned to a commit (keep them current with Dependabot).
- FTPS in this action does not pin the server's certificate; the connection is encrypted, but a certificate is not checked
  against a known one.
- Let the workflow be the one that publishes: the revision of each archive depends on its bytes. A Windows build and the
  workflow's Linux build were compared once (19 November 2026, publish off): all 25 archives and `index.json` were byte for
  byte identical, from the same inputs (the SPFx fork at `c2fd2f5`, the engine at `a1c9e8a`, the templates repo at `845beaf`).
  The archives are deterministic and newlines are normalized, so that should hold, but it is one comparison.

### What was tested, and what was not

Tested on a developer machine:

- The workflow file: structure, manual trigger only, all actions pinned to commits, the FTP secrets referenced only in
  the publish job (this was checked before the switch from SFTP to FTPS; the YAML parses after it).
- `publish_build.sh` against the real checkouts: 25 archives (20 official, 5 `yo`-derived) built and checked in about 40
  seconds; first publish; a 404 for the previous index; revisions kept from a previous index (identical index); entries
  dropped from a release reported; refusal of a non-empty output folder and of an SPFx checkout without `LICENSE`.
- `publish_verify.py` against a local web server: passes on a good copy, catches a tampered archive, refuses plain http for a
  real host.
- An earlier version of the upload was an `sftp` script, tested against a stand-in server. It was replaced by the FTPS action
  once the host turned out to offer only FTP/FTPS (port 22 refused connections, port 21 was open), so none of that applies.

The build job on GitHub, run by hand with publish off (19 November 2026, 58 seconds): every step passed and the upload job was
skipped. Against the real domain, the previous-index request returned 404, so it took the first-publish path. It built 25
archives, and the downloaded artifact is byte for byte the build made on this machine.

The first real publish (20 November 2026, run 44668013489, `include_yo_generators` on):

- The upload job ran against the real server and the verify step passed: 25 archives and `index.json` downloaded from
  `https://downloads.mentenaz-server.com/forge-scaffold` and compared with what was built. The `production` environment was
  accepted on the Free plan.
- The first attempt of the upload job failed before sending anything (`Timeout (control socket)` after 30 seconds); re-running
  only that job succeeded. An earlier build run also failed once, on a connection to the same host on port 443 (four timeouts)
  while it fetched the previous index, and passed on a re-run. The host seems to sometimes not answer GitHub's runners, so
  expect an occasional re-run. A failed upload leaves the old `index.json` in place, because the index goes last.
- Port 22 is closed on this host (from the runners, and for the owner's own machine), so SFTP is not available; the upload
  is FTPS on port 21 with the same account the Forge Super apt deploy uses.

Not tested:

- A second publish over files that already exist (replacing an unchanged or changed archive), and what a client sees while a
  file is being replaced.
- The server sends no `ETag` for `index.json` (the verify step warns), so clients cannot make conditional requests there.
  That is a host setting, not something this repo controls.

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
