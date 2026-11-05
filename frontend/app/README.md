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
cover candidates have separate pages of five and fifty records, respectively.
Local continuation is offered only
when the visible receipt set covers every frozen track and all are received.

The owned `components/ui.tsx` primitives follow shadcn/ui composition patterns
(native semantics, variants and `data-slot` hooks). They use no extra packages,
runtime style injection or external fonts. The bundled Bebas Neue font retains
its license in `assets/OFL.txt`.

## Retained-screen comparison

The Component library in `docs/ui-poc/structure.html` and the confirmed interface
section in `PRODUCT.md` remain the visual authority. The later full-page Artist
and Album decision replaces the earlier Wall/inspector study. The HTML study is
unchanged.

| Screen or surface | Approved direction and connected behavior                      | Difference resolution                                                                                                                                                                                                                                        |
| ----------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Home              | Orbit header, focus search, icon sidebar, Music context        | Retained. Empty queue and future suggestions stay explicit.                                                                                                                                                                                                  |
| Search            | Separate artist and exact-edition results, shared media cards  | Selection is separate from the navigation link. Loading, failed and empty results remain distinct.                                                                                                                                                           |
| Artist            | Full page, coral header, edition cards and paging              | Exact selections remain in application state across route and page changes.                                                                                                                                                                                  |
| Album             | Full page, violet header, exact edition and audio details      | Hero selection and the existing single-album preparation drawer are both available. No provider order becomes a recommendation.                                                                                                                              |
| Download drawer   | Native dialog, output policy, destinations and coverage        | Retained single-album behavior. Escape, initial focus and return focus use the native dialog.                                                                                                                                                                |
| Shared selection  | Up to 25 exact editions, selected profiles and visible removal | New bottom tray and drawer with a step rail (Choose, Prepare, Approve each, Start). Prepares independent plans through one atomic batch. Separate job creation is not presented as atomic. Unknown submission locks selection until saved plans are checked. |
| Request           | Amber review, independent edition/source/JPEG/start controls   | Step rail shows edition, source, cover and start state without approving anything. Added guarded review-next. It starts only the current album and navigates only after success. Source paging now matches the server's five-record bound.                   |
| Queue             | Shared rows and status, attention/active/finished groups       | Retained. Paused, failed and backup-pending are not complete.                                                                                                                                                                                                |
| Notifications     | Header panel, text status and explicit navigation              | Retained. No automatic answer, retry or start.                                                                                                                                                                                                               |
| Decisions         | Saved question, version and separate answer/continuation       | Retained. Answering does not itself run the recovery command.                                                                                                                                                                                                |
| Library           | Location-bound Artists, Albums and Folders                     | Retained real index APIs. Saved observations, incomplete coverage and unavailable reads are labelled. Browsing cannot mutate media.                                                                                                                          |
| System settings   | Core application and Music sections, narrow section picker     | Retained real file-backed configuration and SQLite output policy. Revision conflicts, drafts, saved versus active, read-only and redaction remain authoritative.                                                                                             |
| Recovery          | Version inventory, fresh file preview and exact confirmation   | Retained real restore/deletion/continuation operations. Failed preview hides actions. Recorded bytes are not current disk allocation.                                                                                                                        |
| Component library | Shared primitives, status rows, spacing and type               | Retained. No new theme, font, external asset or runtime style package.                                                                                                                                                                                       |

If saved batch checks find changed plans or a missing job, inspect the saved
requests, then use **Remove inspected batch from selection** to unlock the tray.
This removes only that batch from the tray. It does not change, create or retry
saved jobs. A failed read alone does not enable this action.

The narrow Orbit tools occupy their own header row. They do not cover page content
when the page scrolls. The sidebar uses that same header height.

Loading and failure text remain visible beside manual refresh. Required Request
reads and metadata reads do not allow retained data to authorize commands after a
failed refresh. Library file details can show an explicitly labelled older saved
response; it is not current file evidence. Protected input and pending recovery
entries are view-only. Output and backend settings use different save contracts.

Accepted study details are not new acceptance gates. Agent, TV, Movies and
suggestions remain placeholders. The edition star is deferred to issue #21.

## Connected browser workflows

Use `.\dev.ps1 browser` on Windows or `cargo task browser` on Linux.
`browser-build` builds the connected app and the separate Rust example.
`pnpm --dir frontend run test:browser` runs only the browser checks after that build.
Playwright configuration follows the
[official test configuration reference](https://playwright.dev/docs/test-configuration).

The fixture host binds `127.0.0.1:4174`. It creates private temporary SQLite,
configuration, staging and main/backup roots. Its own router does not mount the
production API. Catalog and transfer input are generated; planning, approval,
normalization, native publication and recovery use the real Rust implementations.
The host accepts no library path or provider credential from the browser.
Configuration validation and saves allow only the generated database path.
Additional asset, cache, TLS, staging and credential references are rejected
before the configuration service can inspect them.
The sample-data harness at port 4173 remains separate.

Desktop and narrow Chromium cases use the built connected app. The narrow
project requests reduced motion. Failure traces and screenshots stay in ignored
`frontend/test-results`. CI installs its own Chromium explicitly. For a permitted
existing local Chromium-family browser, set `DOMUS_BROWSER_EXECUTABLE` to its
absolute executable path; this does not install a browser.

These checks supply generated-fixture evidence, not live-provider, production
authentication, installed-service, other-browser or complete V1 acceptance.
The production API guard and stale-command integration tests remain separate.
