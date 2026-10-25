# Connected Music browser

Build from the repository root:

```powershell
pnpm --dir frontend check
pnpm --dir frontend build:app
```

The separate build writes `frontend/dist-app/index.html` and bundled files in
`frontend/dist-app/assets`. The guarded HTTPS API serves these files. Do not open
the HTML as a local file. Do not use the sample-data server for this app.

The browser reads the identity from `/api/session`. Production uses
`/auth/login`. Development requires explicit host configuration and a valid
development TLS certificate. Missing credentials do not select anonymous mode.

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

Lists use bounded provider/database pages. The setup view loads the first 100
registered roots. The request view loads up to 100 transfer receipts. Source and
cover candidates have separate 50-record pages. Local continuation is offered only
when the visible receipt set covers every frozen track and all are received.

The owned `components/ui.tsx` primitives follow shadcn/ui composition patterns
(native semantics, variants and `data-slot` hooks). They use no extra packages,
runtime style injection or external fonts. The bundled Bebas Neue font retains
its license in `assets/OFL.txt`.
