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

There are two ways to upload, chosen by the `transport` input of the workflow:

- **ssh** (the default): the host allows SSH on **port 24** (not 22), accepts only the `hmac-sha2-256` MAC, and has `tar`, but
  not necessarily rsync or SFTP. The files travel as one tar stream into a staging folder and a script on the server renames
  them into place, so each file appears atomically. See "Uploading over ssh" below.
- **ftps** (the fallback): FTP over TLS on port 21 with a cPanel FTP account limited to the folder, the same way the Forge
  Super repo's `build_installers.yml` publishes its apt repository. Never plain FTP: the password would cross the internet
  unencrypted. Port 22 is closed, and SFTP is not available.

1. Upload the `.tar.gz` files first.
2. Upload `index.json` last, so clients never see an index that points at a file that is not there yet.

## Server

- Serve over `https` only (the engine rejects any other URL) and keep the certificate renewing.
- Send `ETag` and answer `If-None-Match` with `304`. Static files on nginx or Apache do this by default.
- Give `index.json` a short cache lifetime, and the archives a long one (their names include the version).
- The app only needs the index URL. Every archive has its own full URL inside the index, so archives can move later by
  editing the index.

### Uploading over ssh

`tools/publish_upload_ssh.sh` (run by the workflow) sends the release as a tar stream to `~/bin/forge-templates-receive stage`
on the server, which unpacks it into `forge-scaffold/.incoming` (at most 200 MB, plain files named like archives or
`index.json` only, anything else is refused and removed), then runs `forge-templates-receive finish`, which renames every
archive into place and `index.json` last and removes `.incoming`. `forge-templates-receive check` lists the folder and
writes, renames and removes a probe file. The server side is `tools/server/forge-templates-receive.sh`.

The deploy user is `mentefpu`, which is the whole cPanel account (mail, other sites, backups). So the key is tied to that
script in two steps, and it is worth doing the second:

1. **Install the script** and check it works with the key unrestricted:
   `scp -P 24 -o MACs=hmac-sha2-256 tools/server/forge-templates-receive.sh mentefpu@downloads.mentenaz-server.com:bin/forge-templates-receive`,
   then `ssh -p 24 -o MACs=hmac-sha2-256 mentefpu@downloads.mentenaz-server.com "chmod 755 bin/forge-templates-receive"`.
   Run the `ssh_check` workflow (it calls `check`), then a publish with `transport=ssh`.
2. **Tie the key to it.** Run `tools/server/restrict-key.sh` on the server (the command is in its header; it needs the
   account's password). It puts `command="/home/mentefpu/bin/forge-templates-receive",restrict ` in front of the line of the
   key `forge-templates-deploy` in `~/.ssh/authorized_keys`, after making a dated copy of the file. It changes only that
   line, refuses if the key is not on exactly one line or the script is not installed, and does nothing if it is already
   done. From then on the key can run only `stage`, `finish` and `check`, in that one folder: no shell, no other path, no
   forwarding. The client asks for `bin/forge-templates-receive stage`, which the forced command reads from
   `SSH_ORIGINAL_COMMAND`, so the same workflow works before and after. If it ever locks the key out, log in with the
   account's password and copy the dated backup back. cPanel can rewrite `authorized_keys` when keys are authorized or
   deauthorized, so run the script again after any key change (it is safe to repeat).

The host sometimes does not answer GitHub's runners (a connection timeout), so `publish_upload_ssh.sh` tries the whole upload up
to 3 times, 20 seconds apart, each try starting again from `stage` (which clears the staging folder, so a repeat is safe).

The pinned host key (ed25519, `SHA256:sidusN7ytwdejwMac77aKk9aYwajP0mgqmaAos9E+ZQ`) is in the workflow files; only the private
key is secret (`SSH_KEY`). Test with `ssh_check.yml`, which changes nothing that stays.

## What a generator can do, in the index

A panel has to tell a generator that makes a new project from one that is added to a solution before it downloads anything, and
the index entry is all it has. The release writes it as two tags on the entry: `can-create` (the manifest has a `folderFrom`) and
`can-add` (it renders into a solution, or has `repeat` folders). It is done with tags, not with a new field or schema, because
every engine released before 0.3.3 refuses an index with a field it does not know and one with a schema above 2: a changed format
would break the template lists of apps already installed. They show the two as ordinary tags. The index allows 8 tags, so a
generator can have 6 of its own. A new field needs a new file at another address, read by the new engine only; that is not built.

## Publishing with GitHub Actions

The workflow is `.github/workflows/publish.yml` in the templates repo. It only runs when started by hand (Actions, Publish
templates, Run workflow), and it has two jobs:

1. **build** (no secrets). Checks out the templates repo, the engine, the SPFx fork and, when `vite_templates` is not empty
   (default `react-ts`), the `packages/create-vite` folder of the Vite fork (`mentenaz/vite`, ref `vite_ref`), then
   `tools/publish_build.sh`. The create-vite templates become the generators `vite-<name>` (see `docs/vite-findings.md`),
   and the check wants create-vite's copyright line in their `NOTICE.txt`, as it wants Microsoft's in the SPFx ones.
   The default is the 14 that install and build (all but the two Qwik ones); a template left out of `vite_templates` is left out of
   the index too, so list every one you want to keep. The script:
   downloads the index published last time (so unchanged archives keep their revision; a 404 means first publish, anything
   else stops), builds every archive and the index, and checks them before anything can be uploaded: sha256, size and URL
   against the index, no `.git` or `node_modules`, `manifest.json` present, and `NOTICE.txt` with Microsoft's copyright line in
   every official template. The result is a table in the run summary (new, unchanged, CHANGED) and a downloadable artifact.
2. **publish** (only when the `publish` box is ticked; environment `production`; the only job that sees the upload secrets).
   With `transport=ssh` it runs `tools/publish_upload_ssh.sh` (see above). With `transport=ftps`, two runs of
   `SamKirkland/FTP-Deploy-Action` (pinned to a commit) send every archive and then `index.json` last, so a client
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
  is FTPS on port 21. It first used the Forge Super apt deploy account and now uses its own, limited to the folder.

Later that day, three more runs failed in the build job, before anything was uploaded, on the same port 443 request to the
host (about nine minutes of timeouts each). The request now connects within 15 seconds and retries 6 times, and each upload
has up to three tries (a retry step only runs after a failed one; `nick-fields/retry` cannot wrap another action's step, so
the steps are repeated). Both are untested against a real failure: the next run, with the dedicated account, connected
first time and the retries did not run.

The second publish (20 November 2026, run 44687137062), with the dedicated `forgetemplates` account whose root is the
`forge-scaffold` folder:

- Build, both uploads and the verify step passed on the first try. The upload read the state file the first publish had left
  in the folder, so the account is in the right place.
- All 25 archives were rebuilt byte for byte the same and kept revision 1; the upload skipped every one ("File content is the
  same, doing nothing") and `index.json` was unchanged.

The SSH upload (20 and 21 November 2026): a first publish with `transport=ssh` (run 44818476736, key not yet restricted)
and a second through `forge-templates-receive` (run 44868665266) both passed the verify step; all 25 archives are replaced
by rename each time. After the key was tied to the script (the forced command), a publish (run 44869210313) and the `check`
(run 44869428613) passed, and a check on the restricted key twice timed out connecting to port 24 before login (the same
intermittent problem; the next attempts connected). The retry in the upload script was added after that and was tested
only against a stand-in `ssh` that fails a set number of times, not against a real timeout.

Not tested:

- Replacing an archive whose content changed, and what a client sees while a file is being replaced. The next template change
  will do it; if the replace fails, the old archive stays and the old index still works.
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
