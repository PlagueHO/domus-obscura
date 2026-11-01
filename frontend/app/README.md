# Connected Music browser

Build from the repository root:

```powershell
pnpm --dir frontend check
pnpm --dir frontend build:app
```

The separate build writes `frontend/dist-app/index.html` and bundled files in
`frontend/dist-app/assets`. The guarded application API serves these files. Do not open
the HTML as a local file. Do not use the sample-data server for this app.

The browser reads the identity from `/api/session`. `domus serve` without a
configuration starts single-user anonymous HTTP at `http://127.0.0.1:8080`.
This mode needs no certificate or sign-in configuration. Production uses
`/auth/login`. Development requires explicit host configuration and a valid
development TLS certificate. Failed secure configuration never selects
anonymous HTTP.

The host must register library roots, bind private staging, save output settings
and explicitly enable local acquisition. Provider credentials stay on the host.
Search and source assessment read metadata only. The app requires separate
frozen-plan approval, source acknowledgement, exact JPEG approval and an explicit
new-only download command. Commands use same-origin credentials and
`x-domus-csrf: 1`. Failed commands are never retried automatically.
Plan approval uses a POST with no body. Source and artwork approvals use JSON.
Source search and folder assessment have no client timeout, so the host's
120-second operation window remains available.

The request page lists Cover Art Archive images for the approved frozen release.
Search and selected-image download are guarded POST commands. Downloads need
separate permitted-use acknowledgement. The host retains provider evidence and
normalizes each image before the browser can approve its exact JPEG.
The existing `/artwork/caa` preparation route remains cache-only.
No external image URLs are rendered. Local cover upload remains available.

The app displays saved job states and publication receipts. An accepted start
command is not completion. `backup_pending` remains separate from `complete`.
Library browsing and setup views are read-only. Saved recovery questions and
failed or uncertain transfers still require the existing operator recovery
commands; this app does not bypass those controls.

The Recovery navigation opens `#/recovery`. Request pages link to
`#/recovery?job={id}`. The inventory uses 50-record cursor pages across jobs.
It reports recorded bytes, not current disk allocation. Select one retained
version to read a fresh file and identity preview. New deletion requires a
verified preview, an irreversible-deletion acknowledgement and a separate final
confirmation. The command carries the exact preview digest. No batch deletion
or restoration of later displaced versions is offered here.

Scratch cleanup starts automatically only after all destinations verify.
Its status is separate from album completion. Recovery can explicitly retry an
existing incomplete scratch operation or its admitted manifest preparation.
An admitted `pending` scratch entry can be retried even without a saved error.
Preparation retries send operation ID `0`; other retries use the saved operation ID.
Protected inputs and pending publication folders are view-only. Failed reads hide
old inventory and previews. Commands never retry automatically. Historical
publication receipts do not prove that retained files remain available.
Request restore controls query retained inventory for the exact job and location.
They require a matching entry with no deletion intent. Inputs do not consume the
filtered page. Missing or failed inventory reads hide those controls.

Lists use bounded provider/database pages. The setup view loads the first 100
registered roots. The request view loads up to 100 transfer receipts. Source and
cover candidates have separate 50-record pages. Local continuation is offered only
when the visible receipt set covers every frozen track and all are received.

The owned `components/ui.tsx` primitives follow shadcn/ui composition patterns
(native semantics, variants and `data-slot` hooks). They use no extra packages,
runtime style injection or external fonts. The bundled Bebas Neue font retains
its license in `assets/OFL.txt`.
