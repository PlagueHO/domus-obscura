# Local album download with developer HTTPS

This is an explicit local development workflow, not a complete V1 deployment.
It uses the fixed `_anonymous` identity. It needs no Entra registration.
Production remains the default and requires Entra. All listeners stay on loopback.

## Build and create a fresh Windows setup

Use the pinned tools. Run these commands from the repository in PowerShell 7:

```powershell
.\dev.ps1 build
.\scripts\local-setup.ps1 -Directory C:\DomusDev
.\target\debug\domus.exe api-preview C:\DomusDev\service.local.json
```

Choose a new directory whose parent already exists. The script refuses an existing
directory. It creates an empty library, private staging, a local SQLite database,
default output settings and a 90-day ECDSA developer certificate for `localhost`
and `127.0.0.1`. Its directories inherit access for the current Windows user only.
It does not install software, change certificate trust or create provider credentials.
If setup fails, it retains the partial setup for inspection.

Open `https://localhost:8443`. Use the same hostname and port as `origin`.
The application uses the real guarded API, not `frontend/local-test`.
It serves the built browser files from `frontend/dist-app`.
Rebuild and restart after frontend changes. The browser has a Component library.

Open **System** to inspect **Backend configuration**. The real file-backed HTTPS
bootstrap supplies active and saved settings through
`GET /api/system/configuration`. The active snapshot remains unchanged until
restart. Changed saved JSON is explicitly unvalidated; reading it cannot change
the running read-only policy. Secret-file paths and Entra identifiers remain
hidden. Malformed, missing and oversized files return errors. A failed refresh
retains the last values with a stale warning. Inspection creates no database,
cache root or staging directory and starts no provider work.
Set `system_settings_read_only: false` outside the UI to enable editing.
Both the running configuration and saved file must permit a backend JSON save.
Use **Edit backend configuration**, then **Validate backend configuration** or
**Save backend configuration**. These commands retain the draft after failure.
A stale revision requires reload and review; the form never overwrites it.
Secret-file references and Entra identifiers have write-only replacement controls.
Blank replacements keep the existing values. Optional removals are explicit.

Validation reads existing local inputs and checks TLS without provider requests,
SQLite migration, root creation/binding, or activation. Cache and staging roots
must already be set up. A successful save is passively validated and
restart-required, not active. Restart is a separate operator action.
Startup checks all inputs again. Do not use `PreparedServer::load` as a dry run.
Stop the API before manual JSON edits, or follow the
[shared writer lock protocol](agent-operations.md#backend-configuration-writers).

The saved design target is `docs/ui-poc/structure.html`, not the temporary
functional forms. The connected app now uses the approved Orbit header and
icon-only Media navigation, Home focus header, shared queue/status components
and fixed-size artist/album cards. The queue groups attention, active and
finished requests; it initially shows five finished rows. Album preparation
opens in a keyboard-accessible download drawer. A cancellation requires inline
confirmation and remains visible until the server confirms its safe result.
The real queue reads album names and the approved retained cover from the current
frozen plan, not sample data. Failed cover loads show an explicit unavailable
state. Remove and Clear finished on this page only hide completed queue entries.
Clearing includes the finished rows behind Show all on that loaded page.
These shared, version-bound choices survive restart. They do not delete albums,
jobs, approvals or receipts. Finished rows also leave the queue after seven days
from completion. Backup-pending and failed rows remain visible.
The diagnostic `/api/jobs` route still returns hidden jobs.
On schema upgrade, known completion event times are retained. A legacy completed
job with no completion event remains visible for seven days from upgrade; its
unknown historical completion time is not inferred from request creation.
Search groups Artists and Albums with separate errors and page controls.
Artist pages use a canonical-ID lookup through the same catalog client, cache
and durable budget. The UI leaves a one-second gap between its initial distinct
reads, as required by the [MusicBrainz API](https://musicbrainz.org/doc/MusicBrainz_API).
This is scheduling, not a failed-request retry. Longer shared provider backoff
remains visible and authoritative. Optional name, type, country and
disambiguation values use the existing bounded parser.
Album pages can browse editions through
`GET /api/discovery/release-groups/{id}/releases`. This uses a dedicated
release-group browse query, not title-search syntax. Every returned edition must
belong to that exact group. The selector keeps the current exact release visible
across pages, uses the provider's actual cursor and can include unofficial
editions explicitly. Selecting an edition opens that exact release. It does not
create or approve a plan, source or cover. The [MusicBrainz API](https://musicbrainz.org/doc/MusicBrainz_API)
documents group-linked browsing and short release pages caused by its track limit.
No provider ordering is treated as a recommendation. Recommendation criteria
remain undefined. The request uses one amber review surface for plan, source,
cover and start controls. Unapproved source and cover editors open for review;
approved details can be expanded without changing approval. Complete received
inputs are labelled as retained receipts, not a new source acknowledgement.
Transfer and publication receipts remain separate from the review. Failed or
stale required reads disable commands. These screens are a partial port; other
retained V1 work remains open.

Transfer receipts use 50-row keyset pages. The separate authenticated
`GET /api/jobs/{id}/plans/{version}/transfers/summary` reads the frozen track
count and all saved transfer states in one SQLite statement. The browser uses
this whole-plan summary, not the visible page, to identify complete received
inputs. A last uncertain or active transfer still blocks local continuation.
Counts are saved ledger evidence, not proof of current staging bytes,
publication or authority to retry. The worker still checks the live claim,
approvals, complete source and sealed staging inputs. Failed summary or receipt
reads keep commands and page controls disabled. Refresh the request to read
them again. These reads do not scan, contact peers or change transfer state.

**Saved request history** reads
`GET /api/jobs/{id}/plans/{version}/events`. It uses bounded ID-based pages and
an index on job, plan version and event ID. Migration 20 adds that index only;
it does not manufacture events or change any approval or job state. The view
shows only events with the exact saved plan version, oldest first. Global
diagnostic event reads retain the older unversioned events. History reads need
the normal authenticated or explicit loopback-development boundary. There is no
history mutation, resume or retry command.
The local `domus job-history <local-database> <job-id> <plan-version> <after-id> <limit>`
command returns the same JSON event page through a current-schema read-only
snapshot. It never creates or upgrades state. For the first page, use `after-id`
`0` and a limit from `1` through `200`.

The Library screen reads bounded saved-index pages only. **Refresh index view**
repeats that read; it never starts a scan or changes media. Run explicit scans
from System settings. Coverage, last scan errors and retained absence labels
remain visible. A failed read is not shown as an empty folder. A failed refresh
keeps the last response with a warning and disables its page controls.
Changing roots resets folder navigation. Changing any resource key immediately
hides the previous resource while the new read starts, so a previous root,
folder or approval cannot appear as the newly selected resource.

**Check root availability** is a separate, explicit protected GET at
`/api/locations/{id}/availability`. It opens only the configured root through
the existing no-follow component walk and checks the retained native identity
and ancestry. Success returns 204. Missing, replaced, unbound and inaccessible
roots return normal API errors. The check does not scan, register or rebind a
root, change coverage or update the index. Cached browsing remains available
when the check fails. Success is a point-in-time observation, not authority for
later media operations. This check does not check child files.

The Start Download click supplies the explicit new-only publication
acknowledgement. It replaces the extra publication checkbox, not the saved
plan, permitted-source or exact-JPEG approvals. Commands are disabled when
required reads are loading, failed or stale. The server still rechecks all
authority before effects. Provider reads retain the existing durable
[MusicBrainz request limits](https://musicbrainz.org/doc/MusicBrainz_API);
the UI does not bypass limits or add automatic provider retries.

The generated certificate is self-signed. Use a certificate already trusted by
your browser, or review and trust the generated public certificate through your
normal development certificate process. Keep the private key private.
Do not disable certificate validation. Trust is an operator action, not part of setup.
For a verified command-line check without changing system trust:

```powershell
curl.exe --cacert C:\DomusDev\certificate.pem https://localhost:8443/api/session
```

This must return `object_id: "_anonymous"`. Browser commands still require the exact
Origin and `x-domus-csrf: 1`. Cross-origin, remote-peer and forwarded identity
requests cannot activate anonymous access.

## Enable one explicitly selected live source

Live provider use needs separate account and permitted-content approval.
Automated checks do not use your account. Do not put credentials or state in Git.

Create an owner-only JSON credential file outside the library and staging:

```json
{
  "username": "your-account-name",
  "password": "your-account-password"
}
```

The generated setup directory is private. A file created there inherits its
restricted access. On Unix, set the credential file mode to `0600`.
Never put these values into the service JSON, browser, URL or logs.

Add `local_acquisition` to `service.local.json` and restart the API:

```json
{
  "staging_root": "D:\\DomusDev\\staging",
  "credentials_file": "D:\\DomusDev\\soulseek.secret.json",
  "server": "PUBLIC_SERVER_IPV4:PORT",
  "acknowledge_plaintext_login": true
}
```

Replace the server placeholder with the actual public Soulseek IPv4 address and
port. It is not a default or a fallback address. The acknowledgement means you
accept the protocol's plaintext login. The API rejects this configuration in
production. It checks provider selection, native root binding and input-file
separation before enabling it. Browser arguments cannot choose credentials,
database paths, staging paths or native destinations.

Only one local source search, folder read or download runs at a time. Source
search can take about 45 seconds plus connection/login time. No shares, listener,
upload, port mapping, acquisition retry or scheduler starts as a side effect.

## Browser flow

1. Search for an artist or album. Open an exact MusicBrainz release.
2. Select MP3, the configured destination and current output settings.
   Prepare and review the frozen output plan. Approve that plan.
3. Read a selected peer folder or import its bounded listing JSON. Inspect the
   complete track mapping. Separately acknowledge the exact edition and permitted
   download, then approve the digest-bound source review.
4. Select **Find covers for this edition** to list Cover Art Archive images for
   the frozen release. Select an image, acknowledge permission and select
   **Download cover preview**. Or upload a permitted local cover.
   Inspect the normalized JPEG and approve that exact candidate/hash.
5. Select **Start Download** after all approvals. Acknowledge new-only publication.
   It receives, validates, writes fresh metadata, verifies the result and copies
   it into the approved new album folder. Follow actual state in the queue.

Artist search uses MusicBrainz's name, alias and sort-name fields. For example,
an alternate name can return the artist's canonical name. A search match is not
edition proof. The query remains literal; it does not execute user search syntax.
See the [MusicBrainz search fields](https://musicbrainz.org/doc/MusicBrainz_API/Search#Artist).

MusicBrainz can reject requests because of rate limits or service load. The local
service retains provider backoff and reuses valid cached results. The browser
shows the service's minimum wait from the time of the error response. Wait before
trying again; requests do not retry automatically. A paused provider needs review,
not repeated searches. This error does not by itself mean the local setup is wrong.
See [MusicBrainz rate limiting](https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting).

Cover search does not substitute another edition or release-group artwork.
The listing is metadata, not a rights grant. The image download uses the installed
Cover Art Archive adapter, its shared provider budget, bounded cache and guarded
HTTPS redirect checks. Provider URLs and credentials cannot be supplied by the
browser. No provider thumbnail URL is loaded directly by the browser.
Downloaded images pass the normal strict normalization checks before becoming
saved JPEG previews. Select a black or white background for transparent images
when needed. Image problems can save an artwork recovery question; stale plans,
storage errors and network errors remain failures. Preparation never approves a
cover or starts audio acquisition. An empty exact-release listing can use the
local upload option; it does not trigger a hidden fallback.
See the [Cover Art Archive API](https://musicbrainz.org/doc/Cover_Art_Archive/API).

The HTTPS host can select the shared file cache through the optional top-level
`cache` setting. See [CAA cache configuration](../README.md#durable-caa-artwork-evidence)
for root requirements, limits and legacy CLI boundaries. This setting caches
MusicBrainz responses, CAA listings, original images and normalized JPEG variants.
The configured local source reader uses the same exact-release cache.
The cover-download command also uses the shared image-retrieval service.
Its native processor creates the selected JPEG on bounded blocking workers.
The backend rechecks fresh provider evidence and the current plan before
retaining the preview. Retrieval never approves the cover.
It does not change
existing previews or approvals. A corrupt or unavailable selected cache returns
an error; it does not silently switch storage or send another provider request.

Default MP3 output:

```text
library\
  F\
    FIXTURE ARTIST\
      FIXTURE ALBUM\
        folder.jpg
        Fixture Artist - Fixture Album - 01 - First Light.mp3
```

The Rust template rule owns names. Folders are uppercase. Tags retain valid
catalog text, including text replaced only in filenames. Track numbers run
continuously across discs; original disc/track information stays in metadata.
MP3 output is fresh ID3v2.3, with exact artist/album/title, track/disc totals,
release date, MusicBrainz IDs and the approved front-cover JPEG. Encoded audio
must remain unchanged. Every audio frame must pass the actual 320 kbps checks
and complete decoding. Advertised peer bitrate is not sufficient.

The worker retains its fenced claim and private staging OS lock through new-only
publication. It never adopts an existing album folder or overwrites partial,
different or receipted-but-missing files. A verified primary can remain
`backup_pending`. New-only publication exposes partially copied final folders;
it is not an atomic replacement or power-loss acceptance.

## Failure and recovery

No command is automatically retried. A stopped worker keeps all received files,
transfer intent, normalization evidence and failed destination copies.
The page shows the local operation error and durable job/transfer/publication state.
The last-operation message is process-local; receipts and questions are durable.

For foreground shutdown, use Ctrl+C. On Linux, SIGTERM uses the same path.
The server first rejects new local source, download and publication operations.
It closes HTTP connections with a ten-second grace period, then waits for the
current local operation and its result recording. This wait has no forced
timeout: it must not abandon an active receive or publication copy.
A server error also drains before the process exits. A forced process kill
cannot use this path; inspect saved receipts and uncertainty before recovery.
This is not a native Windows service or systemd package.
The local drain regression runs on Windows. Linux signal execution and
service-manager shutdown still need Linux acceptance.

Open **Decisions** in the application navigation for durable questions. Pending
answers and saved answers that still need recovery remain in the paginated inbox.
Open a decision and select one offered answer. **Save answer** only saves that
choice. A separate **Apply saved pause answer** command applies a pause decision.
It restores the saved safe state or cancels according to the saved answer; it does
not claim work, start a download or contact a provider. Requests also have a
**Pause at a safe boundary** control. A live worker must reach a safe boundary
before a pause question appears.

For a cover question, open the linked request. Its cover editor is available while
the current unanswered artwork decision is safely paused. Prepare a permitted
image, inspect the JPEG and approve its exact bytes. Return to the decision,
save **Use approved cover**, then separately apply the approved cover and restore
the safe state. The backend rechecks the plan, destinations, approved JPEG and
transfer safety. Failed answers or recovery commands retain the paused state.
The browser cannot bypass these checks, infer an answer or retry a command.

Generic operator questions without an installed browser recovery action still
require the matching CLI command after their answer is saved.
For a current `publishing` or `backup_pending` request with retained publication
receipts, use **Review publication recovery**, then **Confirm new-only publication**.
This sends `POST /api/jobs/{id}/plans/{version}/publication/resume` with the exact
`normalization_run` and `acknowledge_new_only_publication: true`. Normal browser
Origin and CSRF guards apply. The local runtime shares one operation slot with
downloads and source operations. A 202 response means accepted, not complete.
Read saved job state and publication receipts for the result.

Recovery requires an existing journal for that exact run, a current approved
plan, no live worker lease and the host-configured staging root. It reuses the
same new-only publication implementation as `publish-new-once`. It reads no
account credentials, makes no provider connection and never downloads or
normalizes again. Exact synced files can close an unreceipted crash gap.
Partial/different files, missing receipted files and changed destination bindings
remain errors; they are not overwritten, adopted or recreated. A still-offline
backup remains `backup_pending`. Refresh the request after the operation finishes
to inspect its result before another explicit attempt. No automatic retry occurs.
The CLI remains available for explicit publication before any journal exists.
This browser action is not replacement, restore or cleanup.
An uncertain submission must not be reset.
All fully received, sealed inputs can finish locally without another peer
connection. Supply `source_candidate: null` only for an existing source; an idle
new job needs its exact approved source candidate.

Library browsing remains read-only. A completed copy is not automatically a
fresh index observation. Run `domus scan <database> <location-id>` when the worker
has finished, then refresh the browser's library view.

## Repeatable evidence without live credentials

```powershell
.\dev.ps1 test anonymous_https_approves_exact_album
.\dev.ps1 test anonymous_https_resumes_retained_publication
.\dev.ps1 test complete_mp3_and_flac_albums
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
```

The HTTPS test uses a fresh temporary database, generated catalog, generated
MP3 files and a generated cover. It uses the real TLS listener and command
guards, distinct approvals, sealed input receipts, the local worker and
new-only publication. It independently asserts the full folder/file set,
all required MP3 tags, exact cover bytes, removal of old metadata and unchanged
encoded audio for three tracks across two discs. It does not contact a real
Soulseek account. Separate bound-channel tests receive generated MP3 and FLAC
through local protocol sockets, normalize and publish them.

Passing generated tests does not prove that any arbitrary peer has the exact
edition or permitted content. No new live test was authorized or performed here.
The local browser shell is an initial connected implementation. Narrow-screen
polish, full artwork-recovery UI, scheduling, atomic replacement, service
packages and public acceptance remain outside this delivery.

## Manual setup on Linux or with an existing developer certificate

Use fresh, separate local directories. Keep private staging at mode `0700` and
credentials/private keys at mode `0600`. Build with `cargo task build`.
Run `domus init`, `location-add`, `scan`, `staging-init`, and save the JSON from
`settings-default` with `settings-save <database> 0 <settings-json>`.
Use absolute paths in the configuration:

```json
{
  "environment": "development",
  "database": "/absolute/local/state/domus.db",
  "listen": "127.0.0.1:8443",
  "origin": "https://localhost:8443",
  "certificate_file": "/absolute/local/certificate.pem",
  "private_key_file": "/absolute/local/private-key.pem",
  "browser_assets": "/absolute/checkout/frontend/dist-app"
}
```

The PEM certificate must be valid now, support server authentication, match
`localhost`, and match its private key. Configure the optional acquisition block
only after account approval. Run `domus api-preview <absolute-configuration-file>`.
