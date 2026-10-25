# Domus Obscura

Personal home-media management with Rust, React, and SQLite.

**Status: foundation implementation only.** This is not the complete V1 application and is not ready for public exposure. The approved scope is in [issue #1](https://github.com/PlagueHO/domus-obscura/issues/1).

## Implemented locally

- Shared Rust path validation, bounded pages, job states, and output-template previews.
- Local SQLite migrations, atomic job ownership, and checked state changes.
- Durable questions with offered-choice validation, stale-plan rejection, and no automatic media action after an answer.
- Immutable output settings and exact-release output plans with explicit approval.
- Authenticated MusicBrainz discovery with durable request limits, a bounded cache, and frozen exact-edition evidence.
- Version-bound approvals, indexed-coverage decisions, unique output filenames, and guarded worker claims.
- Claim counters that reject stale workers after lease expiry, including reuse of the same worker name.
- Durable single-peer transfer intent, immutable selected files, bounded progress records, and a received-payload checkpoint.
- Bounded Soulseek server login, selected-peer address lookup, direct control negotiation and durable queue/acceptance ordering.
- An isolated, bounded PNG/JPEG cover normalizer with orientation and explicit transparency decisions.
- Read-only per-frame 320 kbps MP3 and native FLAC validation with unchanged encoded-audio evidence.
- Fresh audio metadata and durable normalization recipes, file receipts and publication handoff guards.
- Private staging with sealed transfer inputs, synced outputs, restart verification and claim-bound normalization.
- Retained JPEG previews and explicit, version-bound artwork approval before normalization.
- Bounded exact-release CAA listing validation and a gated, HTTPS-only artwork transport adapter.
- Bounded durable CAA caching and immutable exact-release cover evidence, retained with each prepared preview.
- Immutable native root identities and ancestry checks for registered media and private staging.
- Isolated Entra ID-token verification and bounded, single-use, browser-bound PKCE login challenges.
- Trusted Entra discovery, bounded HTTPS code exchange, and signing-key refresh with a one-hour validity limit.
- Durable hashed sessions with atomic rotation, expiry, revocation, and token-free session audit records.
- A loopback-only HTTPS API preview with protected commands, same-origin checks, secure cookies, and bounded connections and database work.
- Authenticated event replay with durable IDs, bounded pages, reconnect cursors and active-session checks.
- Read-only file inventory with bounded batches, persistent directory checkpoints, scan leases, and cached observations for unavailable locations.
- Main/backup separation and paginated folder/file queries.
- Visible unsafe-link observations without junction/symlink traversal.
- Rust-generated TypeScript contracts and a neutral React output-preview component.
- One shared task interface, editor tasks, and configured Windows/Linux CI.

An experimental one-shot acquisition worker can process an already-selected, approved source through private normalization and a separate playback-test export. A separate explicit command can copy verified staging into new primary/backup album folders. Atomic album publication, replacement, automated source selection, long-running worker supervision, sharing, recovery UI, native services and production packages remain incomplete. The frontend currently builds a component library, not an application shell. The API preview can complete sign-in and serve protected JSON, but it does not provide the browser application. The final theme and shadcn/ui components remain deferred.

## Try the local test harness

You can test a separate browser harness without Entra, CAA, certificates or live providers:

```powershell
.\dev.ps1 local-test
```

On Linux, use `cargo task local-test` (Linux execution remains unverified). Open **http://127.0.0.1:4173**. Use this exact address, not `localhost`. Stop the process with Ctrl+C. Each start creates a fresh temporary database and generated sample directory. Stopping the process discards that test state.

The harness uses the real Rust output rules, SQLite settings versions, plan preparation/approval, observation checks and read-only indexing. You can edit sample metadata, preview MP3/FLAC filenames, save test settings, save and approve a plan, browse the sample directory and rescan it. Rescanning changes the observation binding, so an old plan cannot be approved without preparing a new plan. Save template edits before saving a plan; plans use the selected saved settings version. Sample release metadata is synthetic and has not been checked against MusicBrainz. The generated library contains a text fixture, not real audio.

Select **Run sample album test** for a local download/name/tag/decode check. The test generates three audible tones as 16-bit, 44.1 kHz mono FLAC files. It downloads them from a short-lived loopback HTTP server, checks each download hash, and uses the real Rust filename, metadata and artwork normalization code. It writes a fresh temporary output tree and verifies the files from disk. An independent tag reader checks the original tag text, continuous track numbers, original disc/track mapping, date, synthetic catalog IDs and embedded cover. The decoder checks every sample against the FLAC MD5. Encoded audio frames remain unchanged; no transcoding occurs during normalization.

The result shows the output folder and a player for each track. Open that folder in your desktop player or use the browser controls. Listen to each full track to confirm that you hear a tone. Automated decode checks do not prove that your audio device or chosen player works. Only the three selected audio files and `folder.jpg` appear inside the album folder. Original downloads and `verification.json` are kept at the temporary test root, above the album tree. The temporary folder remains available while the server runs. A new test replaces the previous result. Copy files you want to keep before rerunning the test or stopping the server. Normal shutdown removes temporary data; forced termination can leave temporary files.

This is a generated sample, not a real album or a Soulseek acceptance test. It does not use the worker transfer journal, approved-job normalization orchestration or production publication. The sample uses fixed FLAC output settings, separate from the plan editor and its saved settings. No provider cache rows or production approvals are fabricated.

This is a development-only Rust example, not an authentication bypass in the application. It does not use the production API router. It binds only to `127.0.0.1:4173` and checks Host, Origin and CSRF headers. It accepts no existing database, library-root or listener arguments. Audio routes serve only the generated test result, never arbitrary paths or indexed library files. It makes no CAA, MusicBrainz, Entra or Soulseek requests. Live acquisition, publication and recovery are not available. It cannot test production authentication or TLS. Do not expose it through a proxy or port forwarding.

The neutral test controls are not the final application design. Node.js builds the browser assets; Rust serves them. `.\dev.ps1 local-test-build` builds without starting a listener. The normal check task includes the harness's Rust and frontend tests.

## Transfer records and contracts

Transfer records are a worker-store foundation, not a downloader. A provider-backed, approved plan can bind one peer and one ordered source file for each selected track. A manually entered output plan cannot start acquisition. The journal records intent before submission. A restart preserves uncertain submissions and rejects a second submission. Received-byte checkpoints do not verify audio quality or metadata.

The blocking `receive_transfer_file` application use case now accepts an already-negotiated Soulseek file connection. It reloads the active transfer under the worker claim, checks the staging database/root, and binds the connection to the selected peer and accepted token. It sends a zero offset and streams exactly the selected byte count into private staging. A completed synced/hash-checked staging receipt is required before the database can record success. Reads recheck claim/state, renew the lease every 30 seconds and enforce a ten-second idle timeout and a 30-minute transfer limit. It never retries or resumes an uncertain input.

The caller must identify the peer through a connection manager before supplying the connection. A protocol token is not peer authentication. The receiver owns and closes the file socket when the expected bytes arrive; it must not wait for uploader EOF. This follows the [Soulseek file-transfer protocol](https://nicotine-plus.org/doc/SLSKPROTOCOL.html#file-messages). Bytes beyond the negotiated file length are not staged. Closed/short streams, wrong peers/tokens, changed claims, other staging databases and repeated receives fail explicitly. Partial files remain for reconciliation; they are not silently deleted or overwritten.

Actual loopback TCP fixtures cover both MP3 and FLAC complete-album receive-to-normalization, including an uploader that waits for receiver closure. These automated tests use no live credentials or peer downloads.

The isolated blocking `SoulseekSession` adapter now supports server login on a caller-supplied socket, selected-username address lookup and outgoing direct `P` connections. It uses experimental client version 177.1, not another client's identity. Messages are limited to 1 MiB. Each reply phase has a 30-second deadline and a ten-second idle timeout; lookup and queue waits accept at most 128 messages. Peer addresses must be public unicast IPv4 addresses. Private, loopback, link-local, shared, documentation, benchmark, multicast and reserved ranges are rejected. A failed lookup closes the session. No account name is generated and no automatic reconnect or retry occurs.

`negotiate_transfer` loads the selected file from the claimed job. It commits `submitting` before sending `QueueUpload`. The peer's upload offer must match the exact selected path and size. Ownership and current approval bindings are checked while messages arrive. The accepted token commits before the five-byte upload-response body is sent. Failed or uncertain negotiation does not reset the transfer to `planned`. Protocol fixtures read SQLite independently after queue and acceptance messages to check this order. Wrong peers, stale claims/observations, malformed offers, remote denial and an injected acceptance-transaction failure cannot produce an acceptance response.

Soulseek login sends the password over plaintext TCP. The protocol-required MD5 fields do not protect that transport. Credentials are not stored in the session, database, logs or serializable contracts. Do not reuse a password from another service. Server error text is not echoed. The server-provided username/address binding is not cryptographic peer authentication. The adapter advertises zero shared files and no distributed-child acceptance. It opens no listener and shares no files.

The uploader can request a reverse `F` connection through the server. The adapter accepts only the selected username, `F` type and exact previously selected IP/port. It connects outward to that address and sends the request's one-byte `PierceFireWall` init with its connection token. The receiver then checks the separate accepted file-transfer token before sending offset zero. A wrong username, address, type, token, malformed request or changed worker claim stops the flow. Failed setup closes both control and server connections without resetting the accepted transfer. No inbound listener or port-forwarding change is needed for this narrow path. It does not implement general inbound/indirect peer management.

`acquire_album` checks the complete immutable source before queuing any new file. An uncertain submission or active transfer blocks the album. Every retained received file must have its matching sealed disk receipt. The approved cover must already exist. Files download sequentially from the selected peer through the real journal, bound file channels and private staging. Complete MP3/FLAC fixtures then run the real audio/tag/artwork normalization. The job reaches `publishing`, not `complete`.

Bounded metadata search is available through the separate command below. One user-approved live 14-track MP3 test passed download, complete audio validation, fresh metadata/artwork normalization and private final-name playback export on Windows. Independent read-only checks verified all output names, tags, embedded JPEGs and file/encoded-audio receipts. The user confirmed clear first-track playback and display of the test cover in the desktop player. This is a narrow local test, not complete Soulseek or production acceptance. The user explicitly accepted an approximate source; the exact pressing remains unverified. Server-session maintenance, complete-edition source matching, general indirect connections, submission reconciliation and production publication remain unfinished.

## Soulseek client extension contract

The Rust client must remain usable beyond the first album test. Its extension
target covers common Soulseek operations, not only the current downloader.
The table distinguishes working code from future work. It is not a claim that
the full protocol is implemented or a change to the approved V1 scope.

| Area                | Current support                                                                           | Extension target                                                                                                                       |
| ------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Account and session | Login, fixed online status, selected-user address lookup, duplicate-login detection       | Persistent session, keepalive, explicit reconnect, away status, user status/statistics and watch list                                  |
| Search              | One bounded global search, public metadata results, outgoing reverse `P` connections      | Concurrent correlated searches, cancellation, user/room/wishlist searches, result filters and ranking                                  |
| Peer browsing       | One bounded selected-folder query and conservative track assessment                       | User information, complete share browsing, persistent correlated browsing and approved folder download selection                       |
| Transfers           | Journaled sequential receive from one selected peer, direct control and reverse `F` setup | Queue status, fair scheduling, bounded concurrency, progress events, cancellation, bandwidth limits and explicit resume/reconciliation |
| Shares and uploads  | Disabled; zero shares advertised                                                          | Explicit read-only share roots, share listings, search replies, upload queues, permissions and limits                                  |
| Social features     | Not implemented                                                                           | Private messages and acknowledgements, rooms, membership, room chat, buddy/ignore lists, interests and recommendations                 |
| Connectivity        | Outgoing public IPv4 only; no listener                                                    | Direct/indirect connection manager, optional listener, port reachability, supported obfuscation and distributed-network participation  |

Follow the current [community-maintained protocol reference](https://nicotine-plus.org/doc/SLSKPROTOCOL.html).
Soulseek has proprietary servers and different client implementations. New
features must use documented wire messages, not invent protocol extensions.
Add interoperability fixtures before calling a feature supported.

**Keep these boundaries when adding a feature:**

- `soulseek/protocol.rs` owns bounded field decoding and wire framing. It has no
  socket, SQLite, credential, worker, catalog or filesystem dependency. Normal
  frames retain their numeric message code. Peer-init frames use their separate
  one-byte code format. Raw file streams and distributed messages need their
  own codecs; never force them through the normal four-byte-code decoder.
- Session and peer adapters own connections, timeouts and operation state.
  The frame reader uses the caller's checked-read function so each network
  read retains the absolute deadline and live ownership checks. The same
  codecs can be used with memory fixtures or a future transport without
  changing the on-wire format.
- Feature modules own message bodies and validation for search, browsing,
  transfers and later social operations. Add typed commands/results there.
  Do not create an unrestricted public raw-message API or put feature rules
  in the frame codec. Keep absent fields and unsupported variants explicit.
- Application use cases own approvals, source selection, transfer journals,
  claims and private staging. The protocol client must not open peer paths,
  write library files, choose an edition or approve a download. Browsing and
  chat must remain usable without a MusicBrainz plan or an acquisition job.
  The current `PreparedSearch` is a Domus album-workflow adapter, not a general
  search restriction in the protocol client.

**Persistent-session prerequisite:** the current blocking adapter allows only
one borrowed peer control channel, and search consumes its session. It is a
short-lived implementation, not a concurrent event loop. Before adding
concurrent operations or social features, give each server/peer socket one
reader and one controlled write path. Route typed events by connection,
username, request/search token and session generation. Use bounded pending
requests, result queues and event subscriptions with explicit overflow errors.
Do not let separate feature loops read the same socket, discard another
operation's messages or reuse late replies after reconnect. The current loops
skip some unrelated messages; that behavior must not become the dispatcher.
Unknown messages must follow a documented per-channel policy, while malformed
messages and terminal session events must fail explicitly.

Keep connection tokens separate from transfer tokens. Preserve deadlines,
compressed-size/expansion bounds, private-result exclusion, public-address
checks, secret-free errors and logs. Future text-encoding compatibility must
not silently repair identifiers or change selected remote paths. Preserve
short-lived APIs until their replacement has equivalent behavior checks.

Reconnect is not permission to repeat a transfer. Cancellation, pause, resume,
retry and upload support each need explicit state transitions and tests for
lost replies, stale ownership and partial files. Never reset an uncertain
submission or bypass the current journal/staging checks. Optional listeners,
uploads, sharing and port mapping stay disabled until explicitly configured
and separately approved. An upload must read only an authorized native-bound
share root; a peer-supplied path is never a native filesystem path.

Test additions with isolated wire fixtures, interleaved/late messages,
malformed/oversized payloads, remote disconnects, cancellation and bounded
resource use. Changes to transfer paths must retain the existing complete
MP3/FLAC acquisition, normalization and playback-export regression tests.
Live interoperability needs separate approval; extending the client does not
authorize another account login, download, upload or network exposure.

## Configurable in-process services

Core operations run in separate Rust services inside the existing processes.
They are not separate network services. `application::services` defines the
provider contracts and routing rules. `infrastructure::services::ServiceHost`
connects those contracts to the installed adapters.

| Operation                                    | Service contract               | Installed provider |
| -------------------------------------------- | ------------------------------ | ------------------ |
| Artist and album discovery                   | `CatalogProvider::discover`    | MusicBrainz        |
| Release metadata                             | `CatalogProvider::metadata`    | MusicBrainz        |
| Artwork listing and original image retrieval | `ArtworkProvider::fetch`       | Cover Art Archive  |
| Approved album acquisition                   | `AcquisitionProvider::acquire` | Soulseek           |

Catalog discovery and metadata have independent provider selections. They share
a catalog adapter because MusicBrainz supplies both. Artwork retrieval does not
normalize or approve an image. Acquisition still needs the frozen plan, source
selection, approval, durable claim, transfer journal and private staging.

API preview, worker, source-search and source-folder configurations accept an optional
`services` object. These defaults preserve the current behavior:

```json
{
  "services": {
    "discovery": ["musicbrainz"],
    "metadata": ["musicbrainz"],
    "artwork": ["cover-art-archive"],
    "acquisition": ["soulseek"]
  }
}
```

An empty list disables a capability. An omitted field uses its default.
Each list allows at most eight distinct provider names. Unknown providers and
providers assigned to the wrong role fail before startup effects. Names in
configuration do not install adapters. Last.fm and Spotify are not implemented.
Soulseek source search currently uses the acquisition provider selection.

The catalog service can call several installed discovery providers in configured
or explicit request order. Each provider keeps its own page, cursor, source and
failure. A failure does not silently trigger another provider or erase successful
results. Callers must inspect `failures`, including when every provider fails.
Filtered pages can be empty and still have a next cursor.

External identities use `{ "provider": "...", "id": "..." }`. Provider IDs are
not interchangeable. An artist filter from one provider cannot be sent to
another without an explicit identity mapping. General metadata is not exact
edition proof. Existing exact-release plans still require verified MusicBrainz
cache evidence and their separate approval.

New adapters must define their own fixed authorities, credential handling,
rate limits, cache policy, provenance and durable write rules. MusicBrainz and
CAA retain their shared durable request budget. The CAA service checks listing
membership before each image fetch and schedules the image after the provider
delay. It does not retry a failed request. A delay above fifteen seconds returns
an explicit backoff error. SQLite work stays on bounded blocking tasks.
Cancellation cannot undo a provider request already running on a blocking task;
its transport limits and durable budget still apply.

### Shared CLI and local MCP boundary

`ServiceRequest` and `ServiceResponse` define typed JSON commands and results.
`ServiceHost::call` executes discovery, metadata, artwork listing and image caching through the
same services. The local CLI exercises this interface:

```powershell
.\target\debug\domus.exe service-call C:\DomusDev\services.json C:\DomusDev\request.json
```

The host file contains an existing absolute local database path and optional
service selections. Keep both files outside media roots and Git. Each file is
limited to 16 KiB. The command does not create a missing database or upgrade an
old schema.

```json
{
  "database": "C:\\DomusDev\\state.db",
  "services": {
    "discovery": ["musicbrainz"],
    "metadata": ["musicbrainz"],
    "artwork": ["cover-art-archive"],
    "acquisition": []
  }
}
```

A discovery request can use the configured providers or select enabled providers
with `request.providers`:

```json
{
  "operation": "discover",
  "request": {
    "query": {
      "operation": "artists",
      "text": "Ramones",
      "offset": 0,
      "limit": 5
    }
  }
}
```

Other requests use `operation: "metadata"` with a provider-scoped `release`,
or `operation: "artwork_list"` with `provider` and `release`. Artwork listing
returns `images: null` when the provider reports missing artwork. It does not
return image bytes, grant rights or approve a cover. These commands can update
provider caches and budgets, but cannot create jobs, approve plans, download
audio or publish files. `operation: "artwork_fetch"` also takes an `image_id`.
It retrieves the listing-bound original into the bounded provider cache. Its JSON
result contains only the hash, byte count and fetch sources, not the image bytes.
`image: null` means the provider reported a missing image. A fetched hash does not
prove that an image is valid or approved.

The local stdio adapter calls this interface without copying provider logic:

```powershell
.\target\debug\domus.exe mcp-stdio C:\DomusDev\services.json
```

Only an operator who can read the private host configuration and database should
start this process. Configure the MCP client to launch this command directly.
Do not put a network proxy in front of it. It has no HTTP endpoint, listener,
browser authentication mode or public MCP access. Client confirmation remains
the client's responsibility. Do not enable live provider calls without separate
approval.

The adapter supports modern MCP revision **2026-07-28** and initialization-based
**2025-11-25** compatibility. Modern requests require `params._meta` fields
`io.modelcontextprotocol/protocolVersion` and
`io.modelcontextprotocol/clientCapabilities` on every request.
`io.modelcontextprotocol/clientInfo` is optional. These fields never grant
application authority. `server/discover` advertises the modern revision and
the four read tools. No handshake is required for modern calls. Unknown modern
versions get `-32022` with the supported version; missing required metadata gets
`-32602`. Modern results include `resultType: "complete"` and server identity.

Legacy clients use `initialize` and `notifications/initialized` before tool
requests. The negotiated legacy revision is 2025-11-25. Legacy results retain
their original shape. Modern and legacy requests can share a process, but
modern requests never inherit capabilities or readiness from the legacy
handshake. See the official
[per-request fields](https://modelcontextprotocol.io/specification/2026-07-28/basic#meta),
[discovery contract](https://modelcontextprotocol.io/specification/2026-07-28/server/discover),
[compatibility rules](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
and [legacy initialization contract](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

The fixed tools are `catalog_discover`, `release_metadata`, `artwork_list` and
`artwork_fetch`. Their arguments match the shared request fields without the
outer `operation` field. The tool name selects the operation. For example,
`release_metadata` takes `{"release":{"provider":"musicbrainz","id":"<release-id>"}}`.
Successful tool results include structured JSON and the same JSON as text.
Image fetches return only a digest and byte count. Provider failures and
backoff remain visible. Reads can update private caches and provider budgets.
No tool approves a plan or cover, controls a job, downloads audio, publishes
media or opens a peer connection.

Each input line is limited to 64 KiB, including its newline. The input channel
holds at most two messages. One provider operation can run at a time. A second
active tool call gets an explicit busy error; it is not queued or retried.
Service result JSON is limited to 1 MiB. An oversized result becomes an explicit
tool error, not a partial result. Output lines are limited to 4 MiB, including
their newline.
Malformed JSON gets a protocol error. Oversized or unterminated input closes the
transport with a nonzero exit. stdout contains only JSON-RPC lines; diagnostics
use stderr.

Cancellation suppresses the affected response after the notification is
processed. It lets the current provider operation finish its bounded request
and durable cache/budget persistence. A transport failure also drains that
operation before returning an error. stdin EOF drains the active operation and
then exits. Cancellation is not an acquisition cancellation, worker stop or
automatic retry.

Host paths, credentials, arbitrary URLs and write approvals are not tool
arguments. Treat provider text as untrusted data, not agent instructions.
Future acquisition tools must address approved jobs through the
existing worker preparation and claim rules, not expose raw downloads.
No network MCP server, SDK, chat agent or additional provider is enabled here.

## Experimental metadata-only source search

Load the selected exact release through the application's MusicBrainz budget and
cache. Use an existing local database. This command creates no job or plan:

```powershell
.\target\debug\domus.exe catalog-release C:\DomusTest\state.db <exact-release-id>
```

Use `domus catalog-plan-prepare <local-database> <release-id> <request-json>`
to prepare a provider-backed plan without running an API listener. The request
uses the same `CatalogPlanRequest` fields as the API: `job_id`,
`expected_version`, `settings_version`, optional `format`, `locations`,
`accepts_incomplete_coverage` and optional `bucket`. The JSON file is limited
to 16 KiB. A new job starts at version 1; its first prepared plan is version 2.
Load the exact release first. Missing, stale or checksum-invalid cache entries
fail without provider HTTP requests. Track names and mappings come only from
the validated cache, never from replacement text in the request. Preparation
does not approve the plan, select a peer, claim a job or start a download.

Then use `domus source-search <absolute-configuration-file>` with this structure.
Replace the placeholders. Do not put credentials in the configuration:

```json
{
  "database": "C:\\DomusTest\\state.db",
  "credentials_file": "C:\\DomusTest\\soulseek-credentials.json",
  "server": "<approved-public-server-ip>:2242",
  "release_id": "<exact-release-id>",
  "acknowledge_plaintext_login": true
}
```

The configuration is limited to 16 KiB. It must reject untrusted write access.
The same owner-only, no-follow credential-file rules as the worker apply.
Both files must stay outside every media root. Obtain separate approval before
using a live account. Soulseek login is plaintext TCP, not HTTPS.

Preparation reads a current-schema SQLite snapshot and a fresh, hash-checked
exact-release cache entry. It does not update cache access times or create
jobs, claims, sources or staging files. The query uses the cached artist and
album title, not user-supplied replacement metadata. Running the search opens
outgoing server and reverse `P` connections. It has a 45-second window, a
256-iteration bound and at most 16 distinct peers. Individual peer setup/read
phases have at most two seconds. It does not open a listener or change sharing,
firewall or port-forwarding settings.

The report includes public paths, sizes, advertised audio attributes, slot/queue
details, individual peer failures and a limit flag. Each response has a bounded,
checksum-checked complete zlib stream. Files from private result sections are
never suggested. Missing attributes remain unknown. A missing VBR flag does
not prove CBR audio. `audio_downloaded` and `edition_verified` are always `false`.
The numeric server address must be resolved and checked for the intended
server separately. The command does not select a server or retry failed peers.

This narrow outgoing path does not cover every peer. A result can omit other
files in the same folder. Track titles, filenames, durations and folder dates
do not prove an exact pressing, complete folder contents, actual audio quality
or download rights. Keep those limits visible. Do not insert provider/source
rows or infer source approval from a search report. The folder query below can
assess a complete bounded listing, but cannot prove an edition. Separate source, plan and exact
artwork-preview approval are still required before the worker can download.

### Selected peer folder assessment

`domus source-folder <absolute-configuration-file>` makes one metadata-only
request to one selected peer. Use the source-search configuration above and add:

```json
{
  "folder": {
    "peer": "<selected-peer>",
    "remote_folder": "<remote-folder-from-search>",
    "format": "mp3"
  }
}
```

The `folder` field is required for source-folder and rejected by source-search.
Use `flac` for a FLAC assessment. Local loading checks the real fresh exact-release
cache, provider selection, credential permissions and remote identifiers. Loading
does not open sockets or mutate the database. Running uses the server's validated
public address for that peer and one direct outgoing `P` channel. There is no
indirect fallback or retry. An unreachable peer returns an error.

The query uses the documented [FolderContentsRequest/Response](https://nicotine-plus.org/doc/SLSKPROTOCOL.html#peer-code-36)
messages, with exact token/folder correlation. One complete zlib stream is
limited to a 1 MiB frame and 4 MiB expansion. At most 64 directories and 1,000
total files are accepted. Subfolders must stay inside the selected remote
identifier. Duplicate/case-colliding directories or files, malformed attributes,
out-of-folder files, truncated responses and excess counts fail explicitly.
Peer identifiers never enter a local filesystem API.

The report returns the listing and a shared Rust `SourceAssessment`. Exact
case-insensitive titles or `artist - title` names can map to release tracks.
Optional continuous track numbers must match release order. No punctuation
removal, fuzzy title match, disc-number guess or format substitution occurs.
Repeated titles need an unambiguous mapping; one file cannot supply two tracks.
Missing tracks, extra or mixed audio, unknown files and ambiguous matches remain
visible problems. Common image/document/playlist assets are listed but are
not selected for download. Only the separately approved cover is used for output.

MP3 attributes must explicitly advertise 320 kbps and non-VBR to match the
requested advertised quality. Missing attributes stay unknown. These attributes
do not prove encoding. Even a complete mapping has `edition_verified: false`,
`audio_verified: false` and `requires_user_review: true`. The report does not
approve a source, prove rights, claim a job, queue audio or create staging.
The worker source acknowledgement and other approvals remain separate.
Automatic exact-edition selection and persistent peer supervision remain incomplete.

### Saved source review

Save only the `listing` object from a permitted `source-folder` report as a local
JSON file. Do not pass the complete report or add an approval field. This file
contains remote identifiers, not local media paths. Then use:

```text
domus source-candidate-prepare <local-database> <job-id> <plan-version> <absolute-listing-json>
domus source-candidate <local-database> <candidate-id>
domus source-candidate-approve <local-database> <candidate-id> <plan-version> <review-sha256> --reviewed-exact-edition-and-permitted-download
```

Preparation saves an unapproved, immutable listing and its assessment against
the exact frozen approved plan. The review SHA-256 binds the complete listing,
peer, advertised sizes, plan and job version. Review that saved result before
approving its exact digest. The flag confirms the user's edition selection and
permission to download; it is not independent edition or rights evidence.
Preparation and approval do not read credentials, claim jobs, open staging,
create transfer intents or contact peers. Neither action approves artwork.

The job must be idle in `queued` or `searching`, with no pending control or
committed source. Destination observations must still match the approved plan.
An incomplete or ambiguous mapping, unknown/different advertised MP3 quality,
wrong digest or stale version blocks approval. Even an approved candidate keeps
`edition_verified: false`, `audio_verified: false` and
`requires_user_review: true`. Complete audio validation remains mandatory.

Listings are limited to 1 MiB and the folder bounds above. A plan retains at most
25 distinct candidates. Repeating an identical preparation returns the same ID,
including after approval. Approval is single-use and cannot be changed or removed.
Retained candidates stay readable after a new plan version, but cannot authorize
that new plan. A worker records its consumed candidate with all source intents
in one claim-fenced transaction. Manual and legacy sources gain no synthetic
review receipt.

The protected API provides `GET/POST
/api/jobs/{id}/plans/{version}/source-candidates`,
`GET /api/source-candidates/{id}` and
`POST /api/source-candidates/{id}/approve`. Preparation accepts the listing
object. Approval accepts:

```json
{
  "plan_version": 2,
  "review_sha256": "<exact-review-sha256>",
  "acknowledge_exact_edition_and_permitted_download": true
}
```

History uses the usual `after` and `limit` parameters, but returns at most five
full candidates per page. Continue after the last returned ID until a page is
empty. Command routes require the exact Origin and `x-domus-csrf` header.
Production still requires Entra. No route starts a worker. The private read-service
MCP adapter does not expose source preparation, approval or acquisition.
These commands require current-schema state and never create or upgrade it.

## Safe job pause and cancel

`domus job-control <local-database> <job-id> <plan-version> <pause|cancel>`
records an immutable operator request for the current job version. The database
must already have the current schema. The command does not create or migrate it.
Read a request with `domus job-control-status <local-database> <control-id>`.

An idle job with no uncertain transfer can stop immediately. A live worker keeps
its claim until a file/transfer boundary. The request blocks new transfer
submissions, source changes and publication handoff. It does not interrupt an
active receive or erase an uncertain submission. Already-submitted work can
still record its exact received bytes. A crashed worker with an uncertain
transfer needs reconciliation before the request can settle; the request is
not authority to reset or retry that transfer. Repeating the same pending request
returns the same ID. A different pending action is rejected.

Pause saves the previous state in a separate control receipt and creates an
`awaiting_input` question with `Resume` and `Cancel` choices. Use the returned
`question_id` with `question-answer`. Then run
`domus job-control-continue <local-database> <control-id> <plan-version>`.
The answer alone cannot claim or resume the job. Continue applies only that
saved answer. `Resume` rechecks the current plan and indexed destination
observations; active-stage work still requires its approved provider-backed plan.
`Cancel` stops the job without new media authority. A queued job can return to
`queued` before preview preparation or approval; that does not approve any work.
Continue does not claim a worker, open staging, contact a provider or start media
work. Start the worker separately after a successful resume.

Requests can stop `queued`, `searching`, `downloading`, `validating` and
`normalizing` jobs. Cancel can also stop an idle `awaiting_input` job. Publication,
`backup_pending` and terminal states are rejected; they need their separate
publication/recovery rules. Stale versions, stale claims and repeated continue
commands are rejected. Operator questions do not supply artwork approval or
replace artwork recovery.

The worker returns a nonzero exit with a saved control ID when it stops at a safe
boundary. Inspect that control; this is not a completed album result. Transfer
receipts, selected sources, staged bytes and normalization receipts stay intact.
A resumed normalization gets a new claim-specific directory and retains the old
attempt. Cancel does not delete staging, recovery data or library files.
`worker-check` remains read-only and rejects a pending request.

The normal protected API exposes paginated
`GET /api/jobs/{id}/plans/{version}/controls`,
`POST /api/jobs/{id}/plans/{version}/controls` with `{"action":"pause"}` or
`{"action":"cancel"}`, `GET /api/job-controls/{id}`, and
`POST /api/job-controls/{id}/continue` with `{"plan_version":2}`.
Commands require the exact Origin and `x-domus-csrf` header. Production still
requires Entra. These routes do not start a worker or expose a public listener.

## Experimental one-shot worker

`domus artwork-preview <local-database> <job-id> <plan-version> <candidate-id>`
writes the exact retained JPEG bytes to standard output. It uses a read-only,
current-schema snapshot. It does not approve the image or create an output file.
Use binary-safe redirection to a new preview file. In PowerShell 7.4 or later:

```powershell
.\target\debug\domus.exe artwork-preview C:\DomusTest\state.db 1 2 1 > C:\DomusTest\cover-preview.jpg
```

The example IDs are placeholders. Check the command exit code and compare the
file's SHA-256 with the candidate's `sha256`. Show that exact JPEG before
`artwork-approve`. Do not substitute the original PNG or a separately encoded
image. A failed lookup writes no JPEG bytes and returns an error.

Run a local check before starting the worker:

```powershell
.\target\debug\domus.exe worker-check C:\DomusTest\worker.json
```

`worker-check` reads the same configuration, private credentials, current approved
plan, destination observations, artwork and complete source rules as the worker.
It uses a read-only SQLite snapshot. It does not create or upgrade the database,
claim a job, record a source, open staging or contact a server. An older database
must be upgraded separately. SQLite can still use its normal WAL shared-memory
coordination files.

The JSON report contains job/plan IDs, job state, track and received-track counts,
whether the source will be recorded, and the playback-export setting. It contains
no credentials, peer names or remote paths. `network_checked`, `staging_checked`
and `claim_acquired` are explicitly `false`. A successful check does not prove
server/account reachability, available disk space, retained staged bytes or
future worker ownership. Another worker can still own the job. `worker-once`
reloads all inputs and performs the full claim/staging checks before network use.

`domus worker-once <absolute-configuration-file>` runs an explicitly selected source through acquisition and normalization. It requires a provider-backed approved plan, current observation bindings and an explicitly approved cover. For an existing `downloading` job, it uses the immutable complete source journal and refuses unresolved transfers. For a `queued` or `searching` job without a source, the optional selection below records the user's complete mapping before any network connection. It does not search for peers or independently prove an edition match. The browser harness does not create these prerequisites. Do not insert source or provider rows manually to make a job runnable.

The worker also requires current-schema state. Upgrade it separately with an
explicit `domus init <local-database>` operation. Startup must not migrate it.

Configuration uses local file references, not inline credentials. This example contains placeholders; replace them before use:

```json
{
  "database": "C:\\DomusTest\\state.db",
  "staging_root": "C:\\DomusTest\\scratch",
  "credentials_file": "C:\\DomusTest\\soulseek-credentials.json",
  "server": "<approved-public-server-ip>:2242",
  "job_id": 1,
  "plan_version": 2,
  "acknowledge_plaintext_login": true,
  "export_for_playback": true
}
```

The `server` value must be a numeric public IPv4 address and nonzero port for the intended Soulseek server. The worker does not resolve a hostname or select a server. The configuration is limited to 16 KiB. The credentials file is limited to 4 KiB and must contain only `username` and `password` JSON strings. Keep it outside all media roots and Git. Use an owner-only file on local disk, with no links or untrusted access. On Unix, use mode `600`. Credentials must be created through a safe local method, never pasted into a chat or command-line argument.

For an initial manually selected source, add this `source` object to the configuration. Replace all example values with the permitted source's actual peer, paths and byte counts. Use the frozen plan's exact release ID, not a release-group ID. Map every audio track exactly once in continuous release order. All files must come from the one named peer. The acknowledgement confirms both the user's edition selection and permission to download; it is not independent evidence of either.

```json
{
  "source": {
    "release_id": "<exact-release-id-from-approved-plan>",
    "peer": "<selected-peer>",
    "acknowledge_exact_edition_and_permitted_download": true,
    "files": [
      {
        "track_number": 1,
        "remote_path": "Shared\\Permitted Album\\01.flac",
        "expected_bytes": 123456
      }
    ]
  }
}
```

The example has one track. A three-track plan needs three entries numbered 1, 2 and 3. Source format must match the approved plan. Invalid, missing, duplicate or reordered entries fail before a claim or staging change. Peer upload offers must later match the exact selected paths and byte counts. Audio quality and complete decoding are checked after download, not inferred from the acknowledgement.

Alternatively, add `"source_candidate": 1` with the exact saved, separately
approved candidate ID. Omit `source` when using this option. The worker checks
the candidate before reading credentials, then rechecks it under the live claim
inside the source-intent transaction. It does not select another candidate or
fall back to a manual source when that check fails.

Loading checks local state, acquires a fenced five-minute worker claim and locks private staging. For a new source, it commits all selected files in one transaction and advances the job to `downloading`. It does not open a network socket. Only `run` starts the explicit outgoing server connection. Invalid credentials and missing artwork approval are rejected before claiming the job. A later preparation or run failure can retain the lease until expiry. Never clear it or reset a transfer to force a retry. Reconcile uncertain remote effects first.

After a source commits, remove `source` and `source_candidate` from the configuration before a later run. Repeating or changing a selection on a `downloading` job fails; it cannot replace the journal or repeat a submission. A failure before the source transaction commits can leave the job in `searching` without files. After the lease expires, the same explicit selection can prepare that job. These rules do not provide automatic remote retry or resume.

With `export_for_playback: true`, the worker also copies the verified normalized files into a new `playback-<run-id>` tree inside private staging. It uses the frozen uppercase folder layout and final filenames. The album folder contains only selected audio files and the exact approved `folder.jpg`. Each copied file is synced and read back against its normalized receipt. No configured library destination is changed. A `complete.json` record sits above the album tree, outside the album folder. Existing or partial playback trees are not overwritten or removed. Failed exports remain for inspection and return an error.

Successful output is JSON with `normalization_run`, `playback_folder` and `publication_pending: true`. Open `playback_folder` in a player to check audible playback. A test export does not complete the job, publish a primary/backup album or implement recovery. Use a fresh temporary staging location for the first album test. The worker does not automatically delete its state or music at exit.

New submissions require unchanged approval bindings. Existing transfer receipts can still be recorded when an index refresh changes those bindings. Uncertain transfers block plan revision and terminal job transitions. Peer reconciliation, cancellation, source retries and destination journals remain incomplete. Private input staging is described below.

The generated TypeScript declarations cover the current JSON requests, read models and errors. Integer bindings use `number` to match JSON serialization, not `bigint`.

## Catalog discovery boundary

The protected discovery routes search artist names or album titles, browse an artist's releases, and read a selected exact release from the [MusicBrainz API](https://musicbrainz.org/doc/MusicBrainz_API). Search text is escaped as a literal phrase. Pages contain at most 25 provider entries. IDs, credited names, join phrases, partial dates, original disc/track order and absent optional values remain distinct. A release-group ID cannot substitute for a release ID. Video, unknown recording type, pregap and data-track cases cannot silently become an audio-only plan.

Release discovery defaults to official albums and EPs. The query flags `include_singles`, `include_live`, `include_compilations` and `include_unofficial` add those choices. The shared Rust policy checks returned metadata as well as provider query filters. Unknown primary types do not become albums. Missing status does not become official. An album search uses `query` and an optional `artist_id`. An artist's release route also accepts `query` to search within that artist.

Release pages report provider `count`, `scanned` and `next_offset` before local filtering. Use `next_offset`, not the number of visible items or the requested limit. A page with no visible items can still have another page. Live/compilation switches can reuse the same cached raw browse page without another request.

`POST /api/discovery/plans` prepares 1 through 25 selected albums per request. Load each exact release first. Send `{"albums":[{"release_id":"...","request":{...}}]}` with a `CatalogPlanRequest` for each existing job. Each album can select its own format and bucket. The command reads only the fresh cache; it never starts provider HTTP requests. All plan writes and events commit together. A missing, stale, invalid or conflicting album leaves every job unchanged. Duplicate jobs, duplicate releases and shared destination folders are rejected. Each resulting plan still needs separate approval. A repeated request with old versions fails without creating more plans; reload those job plans to check an uncertain response.

The HTTPS client uses only `musicbrainz.org`, sends an application/contact User-Agent, and disables redirects and automatic retries. Successful responses require JSON and no unsupported content encoding. Header and body limits apply before parsing; response bodies are limited to 2 MiB. Upstream error bodies do not reach the browser.

Each request holds an OS file lock and a fenced SQLite capability for the same canonical database. The lock stays held through response persistence. A new capability waits at least one monotonic second before HTTP starts. A suspended owner cannot lose that lock because a wall-clock lease expires. A dropped request keeps a conservative 60-second recovery interval. Failure backoff starts at five seconds, grows to 300 seconds, and preserves a longer `Retry-After`. These controls follow the [MusicBrainz rate-limit guidance](https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting). Independent installations do not share a global IP budget.

Saved interval lengths drive a bounded, process-shared monotonic clock. Wall-clock changes cannot shorten request backoff. A new process waits the full saved interval when it first observes that request generation. This can extend backoff after restart. HTTP-date `Retry-After` values use the provider's single valid `Date` header, with a one-second precision margin, not the host wall clock. A missing or ambiguous reference pauses requests. Fixtures check competing processes, owner termination, changed wall deadlines, conservative restart and timer bounds. Real OS suspend/resume and live provider acceptance remain unverified.

Keep the database's `.musicbrainz.lock` sidecar in place while any process runs. Never delete or replace a live lock file. Stop all application processes before a schema upgrade. Migration nine pauses unresolved legacy provider timing because schema eight did not retain the original interval.

Cached raw responses expire after 24 hours. The cache holds at most 64 entries and 16 MiB of raw bodies. Reads verify SHA-256 before parsing. Cache hits do not send a request and remain available during provider backoff.

`GET /api/discovery/status` reports available, active, backoff or paused state. An unsupported or ambiguous `Retry-After` pauses new provider requests across restart instead of retrying early. Investigate the response or legacy upgrade condition before recovery. `POST /api/discovery/resume` requires `{"acknowledge_pause":true}` and the normal Origin/CSRF checks. It clears only that pause, not a supported backoff or live ownership. It does not send a request.

The release-plan command builds track labels from the validated provider response, not browser-supplied metadata. Preparation checks the cached raw response, digest, source, age and exact mapping. Approval freezes the normalized snapshot. Later cache expiry does not change an approved edition. Manual plans remain useful for output previews but cannot start new transfers. Existing transfer receipts can still be recorded for older plans.

MusicBrainz [licenses core and supplementary data separately](https://musicbrainz.org/doc/About/Data_License). Raw responses can include supplementary data. Keep MusicBrainz attribution with displayed metadata. This personal-use integration does not grant cover-art rights.

Local-library match evidence, the browser selection interface, source-peer matching and Cover Art Archive integration remain incomplete. Catalog evidence alone does not prove that a peer has the selected edition. No live MusicBrainz request was used for local acceptance checks.

## Durable CAA artwork evidence

The installed artwork service uses a durable CAA cache. Successful listing and
image bodies have a 24-hour lifetime. Cache reads verify the original SHA-256 and
the complete allowed HTTPS source chain. Image keys include their listing-bound
filename, so changed image formats cannot reuse the old entry. Missing responses
and provider failures are not cached as successful artwork.

The cache retains at most 32 entries and 64 MiB of response bodies plus source
JSON. It removes expired entries and the least recently accessed entries during
successful writes. These are logical cache bounds, not a database-file or
process-memory limit. Frozen cover evidence is retained separately.
The final response, cache write and provider-budget completion commit in one
transaction while the request still holds its OS lock. A failed commit preserves
the uncertain request lease instead of allowing an immediate retry.

Use `service-call` with this request after separate approval of live provider
access. Replace the release and image IDs with a selected exact release and an
image from its listing:

```json
{
  "operation": "artwork_fetch",
  "provider": "cover-art-archive",
  "release": {
    "provider": "musicbrainz",
    "id": "<exact-release-id>"
  },
  "image_id": "<selected-image-id>"
}
```

Then prepare a preview from fresh cached evidence without another network call:

```powershell
.\target\debug\domus.exe artwork-caa-prepare C:\DomusDev\state.db <job-id> <plan-version> <image-id> null --permitted-caa-cover
.\target\debug\domus.exe artwork-evidence C:\DomusDev\state.db <job-id> <plan-version> <candidate-id>
```

The plan supplies the exact release. The command rejects a missing, expired,
corrupt, wrong-release or unlisted image. Strict image normalization still checks
the original bytes. `null` supplies no transparency background; use an RGB byte
array only when that choice is explicit.

Preparation atomically saves the normalized JPEG, event and immutable evidence:
the raw listing and hash, selected image metadata, original image hash, both
source chains, both fetch times and the permitted-use acknowledgement.
The existing exact-JPEG preview approval remains separate. CAA edit approval and
the permitted-use acknowledgement do not prove copyright or user preview approval.
Recoverable image decisions now save a durable question through the CLI and API
preparation paths described below. The selection UI remains unfinished.

The API offers the same cache-only preparation through
`POST /api/jobs/{id}/plans/{version}/artwork/caa`. Its bounded JSON body contains
`image_id`, optional `background`, and `acknowledge_permitted_use`.
The route requires authentication (or explicit anonymous development), exact
Origin and CSRF guards, and an enabled CAA artwork provider. It does not contact
CAA or approve the result.
`GET /api/jobs/{id}/plans/{version}/artwork/{candidate}/evidence` returns the
retained evidence summary through the protected API. Raw listing bytes are not
included in that summary.

Cache eviction or expiry cannot erase a prepared cover's evidence. Preview,
approval and normalization reads recheck retained evidence against the exact
plan and original input hash. Existing legacy and local candidates remain valid
and return `null` evidence; migration does not invent provider provenance.
Schema 14 adds the cache and evidence tables. Stop old processes before an
explicit database upgrade. Read-only diagnostics and `service-call` refuse older
schemas. This implementation did not upgrade the external live-test database.

The endpoint and image membership follow the
[official CAA API](https://musicbrainz.org/doc/Cover_Art_Archive/API).
This is provenance and integrity evidence, not an artwork rights grant.
Production publication and the complete selection UI remain incomplete.

## Persistent artwork decisions

Schema 15 connects artwork issues to the existing persistent question inbox.
Missing covers, missing permitted-use acknowledgement, unapproved previews,
transparency backgrounds, animation, color profiles and unsupported coding or
formats have typed reasons. The CLI and API preparation paths save recoverable
image issues, then return an explicit error with the saved question ID.
These decisions do not accept or repair malformed images. Corrupt cache entries,
stale plans, storage failures and network failures remain errors; they are not
converted into artwork decisions.
The low-level image helper itself still has no database effects.

Use `artwork-check` to pause an idle job that needs a cover or preview approval.
This is a mutating command, not a read-only diagnostic. A missing provider image
does not by itself identify a job; call this check for the selected plan.
The worker also performs this check before claiming new work or opening a socket.
`worker-check` remains read-only and does not create a question.

```powershell
.\target\debug\domus.exe artwork-check C:\DomusDev\state.db <job-id> <plan-version>
.\target\debug\domus.exe artwork-question C:\DomusDev\state.db <question-id>
```

The question retains the current plan version and the safe paused state.
It cannot interrupt a live worker without that worker's fenced claim.
Uncertain submitting or active transfers must be reconciled first.
Repeated checks reuse the current question. A new recoverable preparation issue
updates that unanswered question and its prompt without losing the pause state.
Answered decisions retain their selected candidate and exact JPEG hash.
No operation silently approves artwork, substitutes a cover or skips required art.

Prepare a permitted local replacement or select another verified cached CAA image
with the existing commands. For transparency, pass an explicit RGB background.
Animated, profiled and unsupported images need a supported replacement; this
workflow does not implement frame extraction or color management.
Read the retained JPEG preview and approve its exact hash. Then save the offered
choice and resume the saved state:

```powershell
.\target\debug\domus.exe question-answer C:\DomusDev\state.db <question-id> <plan-version> "Use approved cover"
.\target\debug\domus.exe artwork-resume C:\DomusDev\state.db <question-id> <plan-version>
```

An answer cannot replace preview approval. Answering does not resume the job.
Resume rechecks the current approved plan, destination observations, retained
JPEG/evidence, selected candidate/hash and transfer state. It restores only the
saved safe state and records an event. It starts no network or file operation.
Changed plans and old worker claims cannot reuse the decision.
Download receipts are unchanged. A later `worker-once` can normalize sealed
received inputs in `validating` or `normalizing` without a Soulseek connection.
If all downloads finished before a `downloading` pause, the worker advances to
validation without a new login or download. Normal worker configuration checks
still apply. The job still stops at `publishing`, not production completion.

The guarded API exposes the same operations:

- `POST /api/jobs/{id}/plans/{version}/artwork/check` returns the saved
  `ArtworkDecision`, or `null` when the cover is ready and there is no artwork pause.
- `GET /api/questions/{id}/artwork` reads the issue, safe state and resolution.
- `POST /api/questions/{id}/answer` saves `{"plan_version":2,"choice":"Use approved cover"}`.
- `POST /api/questions/{id}/artwork/resume` accepts `{"plan_version":2}`.

Normal authentication or explicit anonymous development applies. Commands retain
Origin/CSRF guards. Inbox pages and durable events remain the reconnect boundary.
The browser inbox and artwork selection screens still need implementation.
Schema-14 upgrades preserve existing generic questions without assigning invented
artwork context. Stop old processes before an explicit database upgrade.

## Artwork proof boundary

The isolated Rust helper accepts static PNG and supported JPEG bytes. It applies EXIF orientation, preserves proportions, limits each output side to 1200 pixels, and never enlarges the image. It produces one immutable JPEG payload with a SHA-256 digest for future embedding and `folder.jpg` publication. It does not fetch artwork, write tags, or publish files.

JPEG input uses the safe [TurboJPEG binding](https://docs.rs/turbojpeg/1.5.1/turbojpeg/struct.Decompressor.html) and pinned [libjpeg-turbo 3.2.0](https://github.com/libjpeg-turbo/libjpeg-turbo/releases/tag/3.2.0). An independent bounded container walk requires one exact image, valid segment boundaries, an end marker without trailing data, 8-bit baseline/progressive coding, and at most 32 scans. Pixel bounds apply before native decoding. Only RGB, YCbCr and grayscale are accepted. Native decoding must finish without an error or warning. Generated baseline/progressive and color/grayscale tests reject missing end markers and incomplete entropy in each scan, including inputs with a restored end marker. Two earlier pure-Rust decoder candidates silently accepted that damage and are not used for input acceptance.

Transparent pixels require an explicit background color. Opaque images with an alpha channel do not need that decision. The helper composites before resizing to prevent hidden colors from entering the result. PNG input must end with a complete `IEND` chunk and have no trailing data. Both formats reject malformed or ambiguous EXIF orientation instead of guessing. Animated images, ICC-profiled images and unsupported formats return typed decision errors. The preparation adapters persist these decisions as described above. Frame selection and color management remain unfinished; use a supported replacement.

Encoded input and output are each limited to 8 MiB. Source sides are limited to 8192 pixels, the pixel count to 16,777,216, and decoded pixel bytes to 64 MiB. Pixel checks occur before full image decoding. The [decoder allocation setting](https://docs.rs/image/0.25.10/image/struct.Limits.html) is best-effort, not a total-memory guarantee. Decoder metadata, conversion and resize buffers also use memory. The combined API/worker memory target remains unverified.

## MP3 proof boundary

The isolated read-only inspector checks every MPEG audio frame. Each audio frame must be MPEG-1 Layer III at 320 kbps. Sample rate and channel count must stay constant. It checks complete frame boundaries, reserved header values and optional protected-frame CRCs. It then decodes every audio frame with [Symphonia 0.6.1](https://docs.rs/symphonia/0.6.1/symphonia/). Decode errors or incomplete decoded frames cause rejection. Advertised or average bitrate is not acceptance evidence.

One first-frame Xing/Info header is counted separately from audio. Its side information must be empty, and any saved frame/byte counts must match the observed MPEG range. A later Xing/Info frame is rejected. VBRI and free-format MPEG need separate validated support. Bounded ID3v2, trailing ID3v1 and APE framing can be excluded from the MPEG range. Trailer signatures inside a complete audio frame are not treated as tags. Existing tag contents are not trusted.

The result includes the exact MPEG byte range, its SHA-256 digest, format values and physical decoded sample count. The sample count is not a gapless duration estimate. The inspector does not transcode or write files. Generated silence and nonzero spectral fixtures check decoding, every supported sample rate/channel count, padding, CRCs, bitrate changes, metadata counts, malformed inputs and unchanged encoded bytes. A 4,096-frame fixture checks that read requests stay within 64 KiB. Metadata framing is limited to 8 MiB in total. These bounds are not a combined process-memory guarantee.

Private staging and the normalization use case now bind this proof to sealed input bytes and durable receipts. The downloader and publication remain unfinished.

## FLAC proof boundary

The isolated read-only FLAC inspector follows the frame rules in [RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html). It requires one first STREAMINFO block and exact native FLAC framing. An independent parser checks canonical frame/sample numbers, continuous numbering, stable blocking strategy, matching audio properties, complete subframes, zero alignment padding, header CRC8 and frame CRC16. Fixed and linear prediction use bounded 64-bit reconstruction. Every reconstructed sample must fit its declared bit depth, including restored stereo channels, before the playback decoder receives the frame.

Every accepted frame must also decode completely with Symphonia 0.6.1. A nonzero STREAMINFO MD5 must match the decoded audio. An absent MD5 is reported as unverified, not as a successful checksum. The result retains the original STREAMINFO, exact encoded-audio byte range, SHA-256 digest and observed frame/sample counts. No files are written or transcoded.

Generated fixtures cover constant and nonzero audio, verbatim coding, fixed and linear prediction, Rice/Rice2 residuals, escaped residuals, wasted bits, fixed/variable blocking and shorter final blocks. They check CRC reference vectors, MD5, 4/8/12/16/20/24/32-bit independent channels, stereo restoration, malformed coding, truncation and trailing data. Separate 4,096-frame fixed/variable fixtures check multibyte numbers, unchanged encoded bytes and read requests of at most 64 KiB.

Metadata framing is limited to 8 MiB and 128 blocks. Each encoded frame is limited to 16,777,215 bytes. Decoded channel blocks contain at most 65,535 samples each, with at most eight channels. These bounds are not a combined process-memory guarantee. Existing metadata contents, except STREAMINFO, are skipped and never trusted or used to fetch linked artwork. Full metadata-content validation is not provided.

The current decoder cannot accept sample rates above 655,350 Hz or 32-bit stereo coding with a 33-bit side channel. Those cases return explicit errors. Private staging and the normalization use case now bind this proof to sealed inputs. Publication remains unfinished. Same-length concurrent source changes remain outside this read-only inspector's proof boundary.

## Fresh metadata boundary

The stream-based normalizer writes only to an empty, private staging stream. It does not open paths or change the source. It builds fresh ID3v2.3 tags with [id3 1.17.2](https://docs.rs/id3/1.17.2/id3/), or native FLAC STREAMINFO, Vorbis comments and one picture block. Old tags, comments, pictures, ReplayGain and other unrelated metadata are not copied. The FLAC STREAMINFO and all encoded audio bytes remain unchanged, including a separately counted first MP3 metadata frame.

Metadata comes from the frozen exact release's ordered track mapping, not peer tags or sanitized filenames. Continuous track/total values span every disc. Original disc/total and original track numbers remain separate. Artist credits, Unicode, punctuation, release/recording/release-track IDs and all credited artist IDs are retained. Required values must be present and bounded. Missing dates stay absent. ID3v2.3 writes `TYER`, plus `TDAT` only when a full date is known. `TXXX:DATE` retains the exact known partial date. No day or month is invented. The [MusicBrainz Picard mapping](https://picard-docs.musicbrainz.org/en/latest/appendices/tag_mapping.html) keeps recording and release-track identifiers distinct.

Every output embeds the exact immutable `CoverJpeg` bytes that future publication must also use for `folder.jpg`. Text frames use UTF-16 in ID3v2.3. FLAC picture dimensions match that JPEG. Fresh metadata has a total 8 MiB limit.

The normalizer validates the source, hashes the full input while copying only its encoded-audio range, and checks that the copied range matches the original audio proof. It flushes, validates the rewritten audio again, and reads back the complete output to verify its SHA-256. Read/write failures, source changes, nonempty destinations and mismatched written bytes return errors. Any failed staging output is unusable and must not be published.

The stream helper is not a file-publication operation. The private staging adapter now binds input identity, syncs files and journals the returned evidence. Cached provider-backed CAA selection is available through the CLI and API. Automated selection and publication remain unfinished.

## Normalization journal

Migration ten records one immutable normalization recipe for each worker claim generation. The recipe binds the approved exact release, format, ordered transfer IDs, full input lengths and hashes, track-tag hashes and JPEG. File names use controlled track numbers inside a distinct `normalization-{id}` staging directory. Peer paths do not become local file names.

Each audio receipt must match its frozen input, tags, cover, format and normalization profile. Identical receipt retries are safe. Different receipts conflict. A separate receipt confirms the exact `folder.jpg` bytes. The job cannot enter publication until every track and the cover have receipts. Generic state changes cannot bypass these steps.

Recipes, file bindings and receipts remain after restart or claim expiry. A new claim receives a separate attempt and directory. A stale claim cannot write either attempt. Index changes stop new work and publication handoff, but do not prevent saving receipts for work already issued. Recipe creation, receipts and final handoff commit atomically with their events.

The journal records worker evidence; it does not itself open files, make staging private, sync storage, approve artwork or publish an album. The adapter below supplies private staging and file synchronization. Isolated generated-media checks cover MP3 and FLAC, schema-nine and schema-ten upgrades, reopen/reclaim, receipt bindings, immutable rows and injected transaction failures.

## Private staging and normalization

Migration eleven adds a retained database identity and private-root reservations. Registering a media location rejects overlap with a reserved staging root. A staging root must already exist, have private permissions and contain no unowned files. Its `owner.lock` binds it to one database and holds an exclusive OS lock. Keep this file in place while any worker runs. A missing marker on a reserved root requires explicit recovery.

Roots and files open through no-follow directory handles. Permission checks use those same handles. Windows checks the actual process SID and a strict set of supported owner/DACL forms. Unknown ACL forms fail closed. The safe descriptor binding is [windows-permissions 0.2.4](https://docs.rs/windows-permissions/0.2.4/windows_permissions/), last published in 2021, not a newly maintained dependency. [WinSafe](https://docs.rs/winsafe/0.0.29/winsafe/) reads the process token. Unix checks the effective UID and permission bits. Only the Windows path has run locally.

Inputs use controlled transfer IDs, not peer paths. The adapter requires exact stream completion, syncs and hashes the file, then saves a bounded immutable receipt sidecar. A short, oversized or interrupted stream cannot become a received input. Restart reconciliation requires both that receipt and the matching file bytes.

The normalization use case checks database identity, ownership, the frozen recipe and current destination observations before new file effects. It reads bounded work pages, renews claims during long I/O, writes fresh private outputs, syncs them and reads back complete hashes before saving receipts. Exact interrupted outputs can be verified without rewriting them. Partial, different or corrupt files remain unchanged and cause an error. The cover must also be verified before the durable Publishing handoff.

The acquisition worker uses this application use case with the exact durably approved artwork. Normalization itself does not publish, replace or delete library files. Native root binding is separate from the database marker. Linux execution, directory synchronization on Windows, disk quotas and power-loss acceptance remain unfinished.

## Explicit new-album publication

Migration 16 adds retained destination and file journals. The separate
`publish-new-once` command uses only completed, verified normalization and the
current approved plan. It needs no provider credentials or network connection.
The acquisition worker does not invoke it automatically.

Save this configuration outside media roots. Use only generated test roots until
you separately authorize writes to your library:

```json
{
  "database": "C:\\DomusTest\\state\\state.db",
  "staging_root": "C:\\DomusTest\\scratch",
  "job_id": 1,
  "plan_version": 2,
  "normalization_run": 1,
  "acknowledge_new_only_publication": true
}
```

Use the run ID returned by normalization. The job must be `publishing` or
`backup_pending`. Its previous worker lease must have expired.
The command requires an existing current-schema database. Upgrade state
separately; publication and status commands do not run migrations.

```powershell
.\target\debug\domus.exe publish-new-once C:\DomusTest\publish.json
.\target\debug\domus.exe publication-status C:\DomusTest\state\state.db 1 2
```

All destination/file intents commit before library writes. Publication checks
the saved native root identities, current observations, private staging lock,
approved JPEG and retained audio hashes. It creates only new album folders.
Each file uses its final frozen name. Copies use bounded buffers and full
read-back hash verification. `folder.jpg` is the exact approved embedded JPEG.
File receipts and destination identities remain after restart.

An existing unbound album folder always blocks publication, even if its bytes
match. A folder created just before an interrupted identity receipt is retained,
not adopted. Partial or different files are never overwritten. A missing file
with a saved receipt is not recreated. An exact synced file without its receipt
can be verified on retry. Unexpected files, links, changed root/album identities
and case-colliding paths block completion.

The primary is copied first. An unavailable backup leaves a verified primary
and `backup_pending`, with its error in publication status. Retry the same
command after restoring the original backup root. Retry uses the same
normalization run and does not download, normalize or delete anything.
Before completion, the command verifies all successful destinations again,
with the primary last. Generic state changes cannot bypass the journal.
The protected `GET /api/jobs/{id}/plans/{version}/publication` route returns
bounded destination status. There is no publication mutation API or browser UI.

**Limit:** files copy directly into the final album folder. Partial folders can
be visible during copying and after failure. This is not an atomic whole-album
switch or production acceptance. File synchronization runs before receipts,
but Windows directory durability and power-loss/reboot recovery are not proven.
Replacement, retained old-album recovery, restore and scratch cleanup remain
unfinished. Staging and all partial copies are retained.

## Artwork approval

Migration twelve retains cover candidates and one immutable artwork approval per plan version. Each candidate binds the original input hash, an explicit transparency background, a normalization profile and the exact JPEG bytes. CAA candidate source links use the frozen release ID and a bounded decimal CAA image ID. Schema 14 adds the verified cache and immutable listing/image evidence described above. The older trusted-worker preparation method remains for legacy receipts and isolated fixtures; it does not supply verified CAA evidence.

For a local test without CAA, supply a PNG or supported JPEG that you have permission to use. The local command makes no provider request. It reads a bounded, absolute, no-follow file and retains a content-based `urn:domus:local-artwork:sha256:...` source. It does not retain the local filename or claim provider provenance, copyright permission or exact-release identity. The provider-backed album plan and all existing approval checks remain required.

```powershell
# Use an existing job and its current plan version.
.\target\debug\domus.exe artwork-local-prepare C:\DomusTest\state.db 1 2 C:\DomusTest\permitted-cover.png null --permitted-local-cover
```

The final flag acknowledges permitted use. `null` means no transparency background. Supply `'[255,255,255]'`, for example, when the source requires a white background. Preparation returns candidate metadata and its exact normalized JPEG hash. It does not approve the cover. Inspect the JPEG through the image route below, then use the normal approval route or this local command with the returned candidate ID and preview hash:

```powershell
.\target\debug\domus.exe artwork-approve C:\DomusTest\state.db 1 2 3 <preview-sha256>
```

Different artwork after approval needs a new plan. The local command does not select a music source, download an album, publish files or remove the live-download approval requirement.

The protected routes are:

- `GET /api/jobs/{id}/plans/{version}/artwork`: bounded candidate metadata pages.
- `GET /api/jobs/{id}/plans/{version}/artwork/{candidate}/image`: the exact JPEG preview, not an arbitrary file.
- `POST /api/jobs/{id}/plans/{version}/artwork/approve`: `{"candidate_id":123,"sha256":"..."}` with the normal Origin/CSRF checks.

Approval rechecks the current plan and destination observations. It requires the exact preview hash. Repeating the same decision does not add another event. A different selection needs a revised plan. Old approval never authorizes that revision. Image reads check the saved hash, JPEG framing, decoding and dimensions without re-encoding the payload.

New normalization, resumed file effects and final handoff require the approved JPEG's exact hash, size and dimensions. Schema-eleven upgrades retain existing recipes and receipts, but do not invent artwork approval. An interrupted recipe can accept approval only for its original cover.

The [CAA API](https://musicbrainz.org/doc/Cover_Art_Archive/API) uses release-specific listings and redirects. Its edit-system `approved` flag is not user approval or a cover-art license.

The listing parser binds the response to one exact release. It preserves ordered image IDs, edit IDs, types, comments and separate provider approval flags. IDs remain decimal strings without JavaScript precision loss. An image request must come from that validated listing. It never substitutes a release group, another release, a thumbnail or an arbitrary source URL. The parser accepts at most 200 images and 2 MiB. Text and type counts have separate limits. Missing or inconsistent metadata returns an error rather than inferred values.

The transport adapter makes only HTTPS requests. It starts at `coverartarchive.org`. Explicit redirect checks allow only the same release's archive item and exact filename at `archive.org`, `s3.us.archive.org` or a numeric `ia` shard under `us.archive.org`. At most four redirects are permitted. HTTP downgrade, changed items/files, credentials, queries, fragments and cycles fail. Legacy HTTP links in listing metadata become canonical HTTPS requests; an HTTP redirect is never followed. Unsupported archive routes fail explicitly.

Each hop uses a new single-use capability from the existing catalog OS lock and monotonic budget. CAA and MusicBrainz conservatively share this budget and pause state. Automatic redirects, retries, proxies and content decompression are disabled. Response headers are bounded. Listing bodies are limited to 2 MiB and original image bodies to 8 MiB, including chunked responses. Only JSON listings and JPEG/PNG image content types are accepted. Images still require strict normalization; a content type and transport hash are not image validation. `Retry-After` survives failures. Unsupported intervals pause catalog requests. Each hop has a 15-second timeout and the complete operation has a 30-second limit.

The installed artwork service now connects this transport to the durable CAA cache. CLI and API preparation freeze the listing, original image hash and complete source chains with each preview. The worker uses the separately approved preview through its existing normalization path. No automatic worker fetch is enabled. Unsupported-image decisions are persistent; browser selection remains unfinished. No live CAA request was used for these checks.

## Build tools

Use Rust 1.99.1 with its bundled Cargo, Node.js 26.12.1, and pnpm 12.15.0. Node.js and pnpm are development/build tools only. Native JPEG builds need CMake 3.24 or newer and a native C compiler. Windows builds also need installed MSVC C++ tools and a Windows SDK. CMake included with Visual Studio C++ tools is supported.

The task interface restores the official libjpeg-turbo 3.2.0 source archive on the first native build. It verifies the pinned SHA-256 before extraction. It builds a static library without SIMD, copies its notices, and caches the result by host and source under `target`. No global installation occurs. Later tasks reuse the cache without a download. Use the task interface instead of raw Cargo build/test commands so this native dependency is configured. Cross-compilation is not supported by these tasks.

This software is based in part on the work of the Independent JPEG Group. Include the cached native library's `licenses` directory with any binary distribution. It contains the unmodified libjpeg-turbo license and IJG notices.

Windows builds use a minimal local patch of `file-id` 0.2.3. It exposes the existing full-resolution query through a borrowed file handle instead of reopening a path. The source, verified archive digest, upstream revision and patch description are in `third_party\file-id-0.2.3`. Include its unmodified `LICENSE-MIT` and `LICENSE-APACHE` with Windows binary distributions. Application code contains no unsafe blocks.

On Windows:

```powershell
.\dev.ps1 bootstrap
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
```

The launcher finds an installed complete Visual Studio environment. It does not install tools or change the host defaults. It checks the installed stable Rust alias against the exact required version before use.

On Linux:

```sh
cargo task bootstrap
cargo task check
cargo task build
cargo task smoke
```

Use the pinned Rust toolchain. Do not use beta/nightly tools, npm, npx, or a second frontend lockfile.

## Inner-loop tasks

```sh
cargo task test templates
cargo task contracts
cargo task fmt
cargo task scale
```

Use the Windows launcher instead of the Cargo task prefix on Windows. The scale task checks one million generated indexed records and pagination. It does not prove physical scan performance, service behavior, transfer performance, or the combined 512 MiB memory target.

## Local command boundary

The executable provides version, contract export, database initialization, versioned output settings, job/plan records, location registration/details, read-only scanning, paginated browsing, and a loopback HTTPS API preview. Run it without arguments to see its required arguments; that exits with an explicit usage error.

Use only isolated fixture roots until existing-library access is separately approved. Keep the SQLite database outside all configured media roots. Network prefixes are rejected before SQLite opens a file. The resolved file or parent is also checked. Windows local extended-length paths are accepted. Physical mapped-drive checks remain unverified; select a known local disk.

Browsing returns indexed observations, not proof of current file presence. Use the location command to check coverage, observation time, and the latest scan error. A missing location leaves cached entries intact and marks the scan incomplete.

Scanning uses [cap-std directory handles](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html), not checked paths followed by ambient reads. It opens the filesystem anchor first, then opens each root and observation component separately with [no-follow directory access](https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/trait.DirExt.html#tymethod.open_dir_nofollow). Windows handles deny delete sharing. Directory iteration and entry metadata stay tied to the open directory. All Windows reparse points are excluded from traversal, not only symbolic links. Registration also compares the opened root with its reopened canonical name before saving that name. Links remain visible as unsafe observations; an unsafe queued directory makes coverage incomplete. Isolated tests check directory replacement, ancestor renames and unchanged outside sentinels. A physical fixture checks 192 directories and 1,536 files across batch boundaries. This is not large-library or memory acceptance evidence. There is no raw file-serving endpoint.

## Native root binding

Migration thirteen retains immutable native identities and ancestry for media and staging roots. Windows uses the full 64-bit volume serial and 128-bit file ID from the actual directory handle, without a short-ID fallback. Unix uses handle-based device/inode values. Registration rejects overlapping native ancestry and a root that contains the database through an alias. Scanning and staging check the retained binding before work. A different directory at the old pathname cannot erase cached presence or gain authority through a copied `owner.lock`.

New roots receive bindings during registration. Upgraded roots require an explicit comparison and binding. Stop all application processes before upgrading. Verify that each root is the intended existing directory before running these commands. Use the actual location ID and the exact identity returned for that root:

```powershell
.\target\debug\domus.exe root-identity C:\Media
.\target\debug\domus.exe location-bind state.db 1 "<expected-native-identity>"
.\target\debug\domus.exe root-identity C:\DomusStaging
.\target\debug\domus.exe staging-bind state.db C:\DomusStaging "<expected-native-identity>"
```

These commands save database bindings only. They do not change media or replace the owner marker. Bind every configured legacy root before scanning, staging or registering another root. A wrong identity fails. An existing different binding cannot be replaced by these commands. The location read model reports `native_identity`, or `null` when binding is required.

Native IDs can be reused. This is not protection against cloned volumes or all mount/alias changes. Isolated Windows fixtures check reopen, rename, replacement, legacy upgrades, CLI binding and copied markers. Physical volume aliases, ReFS/SMB behavior, Linux execution and power-loss acceptance remain unverified. Dynamic root relocation needs a separate recovery design.

## Local output plans

Plan commands save and approve records only. They do not contact MusicBrainz or Soulseek. They do not download, create, or replace media. A plan approval is not permission to overwrite existing files.

Save output settings from a JSON file:

```json
{
  "format": "mp3",
  "template": {
    "folder": "<LETTER>\\<ALBUM ARTIST>\\<ALBUM>",
    "filename": "<Track Artist> - <Album> - <Track:00> - <Title>.<ext>"
  }
}
```

The first settings save expects version `0`. Later saves must supply the current version. Old plans keep their saved settings. Settings input is limited to 16 KiB.

```powershell
.\target\debug\domus.exe settings-save state.db 0 settings.json
.\target\debug\domus.exe settings state.db
.\target\debug\domus.exe job-create state.db
```

A manual plan request contains `job_id`, `expected_version`, `settings_version`, `edition`, `locations`, and `accepts_incomplete_coverage`. The edition contains a canonical lowercase MusicBrainz `release_id`, `album_artist`, `album`, optional `bucket`, and ordered `tracks`. Each track contains `artist`, `title`, optional `disc`, and optional `disc_track`. A manual request has no provider evidence and cannot start acquisition. The protected discovery command adds a validated provider snapshot, including available dates and IDs. Artwork, source-peer proof and final file fingerprints remain unfinished.

The optional request field `format` selects `"mp3"` or `"flac"` for this album. If it is absent, the plan uses the saved default. The selected format is saved in the plan without changing global settings. This selection does not start a download or transcode audio.

Use the location IDs from location registration. Include the main location exactly once. Plans allow 1 through 1000 tracks and at most 16 distinct locations. Plan JSON input and stored plans are each limited to 2 MiB. The ordered track list sets continuous output numbering.

```powershell
.\target\debug\domus.exe plan-prepare state.db request.json
.\target\debug\domus.exe plan state.db 1 2
.\target\debug\domus.exe plan-approve state.db 1 2
```

Use the IDs and versions returned by the commands. New jobs start at provisional version `1`. Preparing or changing a plan advances its version and clears its approval. Active jobs cannot be edited.

Incomplete index coverage requires an explicit decision in the request. A changed scan or scan progress invalidates the saved observation binding. Prepare and approve a new preview in that case. Future publication must also check actual files; index approval never proves current file contents.

## Next integration boundary

The inspected Soulseek engine follows links in its share scanner. Do not adopt that scanner unchanged. Reuse permission is user-confirmed, not independently verified. Record notices and prove a safe narrow integration before importing upstream source.

See PRODUCT.md for product boundaries. See AGENTS.md and .github/copilot-instructions.md for development guidance.

## Authentication proof boundary

The OpenID Connect adapter verifies signed ID tokens for one configured Entra tenant/object-ID pair and application ID. It checks the issuer, audience, RS256 signature, expiry, issue time, not-before time, optional token version, and the saved nonce. It rejects other users and returns redacted errors. The [Microsoft claim reference](https://learn.microsoft.com/en-us/entra/identity-platform/id-token-claims-reference) defines these identity claims.

Pending login state is limited to 128 challenges with a five-minute lifetime. Each challenge uses PKCE S256 and a separate browser-cookie binding. Consumption is single-use. These secrets stay in memory, not SQLite. A service restart requires a fresh sign-in.

The adapter uses trusted, tenant-specific discovery and a fixed Microsoft HTTPS authority. It requires the expected authorization and token endpoints. Redirects and automatic retries are disabled. Responses are limited to 512 KiB. Keys refresh every ten minutes. New sign-ins stop if trusted signing metadata reaches one hour without a successful refresh. Provider keys never come from browser input.

The callback consumes browser-bound state before code exchange. It uses the saved PKCE verifier and nonce. Sessions store only a SHA-256 credential hash in SQLite. Session expiry cannot exceed the signed token expiry. Sign-in rotates the previous browser session in one transaction. Failed rotation leaves the previous session intact. Session creation and revocation are audited without credentials.

The [OpenID Connect library](https://docs.rs/openidconnect/4.0.1/openidconnect/) supplies protocol and signature validation. The flow follows the [Microsoft authorization-code and PKCE reference](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow). Isolated private-CA fixtures check discovery, code exchange, signed identity rejection, HTTPS access, session rotation, command protection, durable decisions, and logout. Live Entra configuration and MFA policy remain unverified.

The RSA dependency has a [private-key timing advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html). Application code uses public-key verification only. Private RSA keys are generated in memory for isolated tests and are not saved. Do not use this dependency for runtime private-key operations. A complete dependency/security review remains required before public exposure.

## Loopback HTTPS API preview

This is an incomplete integration surface, not a deployment package. Public binding is rejected. Windows private-file ACL validation, browser assets, native service operation, and public acceptance remain unfinished. Do not use live credentials until the private-file boundary is reviewed.

The `api-preview` command needs an existing local database, a certificate chain and its matching private key. Production also needs an Entra Web callback at `<origin>/auth/callback`. It does not create an app registration, change DNS or generate a certificate.

### Local development without authentication

Set `"environment": "development"` explicitly to use the fixed `_anonymous` identity. Do not supply Entra IDs or a client-secret file in this mode. It makes no Entra discovery, sign-in or key-refresh requests. This is the real API router, with the same database operations and command checks as production.

Use a developer certificate in PEM format with a matching PEM private key. A self-signed localhost server certificate is supported. It must be valid for the configured hostname and date. Use a certificate that your browser trusts, or trust only your own developer certificate in your local test client. Never disable certificate validation globally. No machine certificate-store changes are made by the application. TLS validation and reload checks stay active in both environments.

Example development configuration:

```json
{
  "environment": "development",
  "database": "C:\\DomusDev\\state.db",
  "listen": "127.0.0.1:8443",
  "origin": "https://localhost:8443",
  "certificate_file": "C:\\DomusDev\\localhost.pem",
  "private_key_file": "C:\\DomusDev\\localhost-key.pem"
}
```

Create an isolated local database and start the API after saving the configuration and certificate files:

```powershell
.\target\debug\domus.exe init C:\DomusDev\state.db
.\target\debug\domus.exe api-preview C:\DomusDev\development.json
```

`GET https://localhost:8443/api/session` returns `object_id: "_anonymous"` and `tenant_id`/`client_id: "_development"`. This identity is not an Entra principal. It has no persisted session, token or login cookie. The existing `expires_at` field is a fixed year-9999 sentinel, not a signed-token lifetime. Identity headers, bearer tokens and cookies cannot select another developer identity. Login returns to `/`; the callback is unavailable and logout reports that anonymous development access cannot be signed out.

Development requires a direct loopback connection. The origin must use `localhost` or a literal loopback IP address. Remote peers, external hostnames, cross-site requests and nonmatching Origin headers are rejected. Forwarded headers cannot grant access. Commands still require the exact configured `Origin` and `x-domus-csrf: 1`; no CORS access is enabled. Do not expose this mode through a proxy or port forwarding. Use generated test data until live-provider or existing-library access is separately approved.

The generated-album browser harness above remains a separate HTTP-only fixture tool. This HTTPS development mode enables the real API without Entra; it does not yet install the production browser application or implement Soulseek acquisition.

### Production authentication

Production is the default when `environment` is omitted. Set `"environment": "production"` explicitly in production configuration. Missing or invalid credentials fail startup; they never select development mode. Production always checks the signed Entra identity. A development certificate does not disable authentication.

Store configuration and secrets outside Git and every media root. Use absolute paths. Replace the ID placeholders with the configured lowercase UUIDs. The origin port must match the loopback listener.

```json
{
  "environment": "production",
  "database": "C:\\ProgramData\\DomusObscura\\state.db",
  "listen": "127.0.0.1:8443",
  "origin": "https://localhost:8443",
  "tenant_id": "<tenant-uuid>",
  "object_id": "<object-uuid>",
  "client_id": "<client-uuid>",
  "client_secret_file": "C:\\ProgramData\\DomusObscura\\client-secret.txt",
  "certificate_file": "C:\\ProgramData\\DomusObscura\\certificate.pem",
  "private_key_file": "C:\\ProgramData\\DomusObscura\\private-key.pem"
}
```

Use absolute Linux paths on Linux. Secret and private-key files must have owner-only permissions there. The client-secret file contains one nonempty UTF-8 line. Configuration is limited to 16 KiB, the secret to 4 KiB, the certificate chain to 256 KiB, and the private key to 16 KiB. Stable links and nonregular files are rejected. Concurrent hostile file replacement remains outside the current proof.

```powershell
.\target\debug\domus.exe api-preview C:\ProgramData\DomusObscura\service.local.json
```

Startup validates configuration and TLS before binding. Production also requires successful trusted Entra discovery. TLS loading checks the key pair, certificate chain, hostname, and current validity. HTTPS clients independently check certificate trust. TLS files are checked again every minute. Invalid replacement material is rejected without activating it. Reload failures are reported to stderr. This is manual certificate loading, not ACME automation.

The listener permits at most 64 complete connections, including handshakes and idle connections. TLS and HTTP-header timeouts are ten seconds. Header count is limited to 64. The HTTP parser buffer is limited to 32 KiB. Application headers are limited to 16 KiB and URLs to 8 KiB. Application requests are limited to 32 concurrent operations and 25 seconds. Database work uses at most four admitted blocking tasks and one short-held store lock. The JSON body limit is 2 MiB; settings requests have a 16 KiB limit.

Sign-in starts are limited to twelve per minute for each direct client IP. The rate table holds at most 256 active IPs. Forwarded headers do not set the client IP. Login challenges and active sessions each have a separate 128-entry limit.

The session and login cookies use `Secure`, `HttpOnly`, `SameSite=Lax`, and the `__Host-` prefix. Commands require the exact configured `Origin` and `x-domus-csrf: 1`. No CORS access is enabled. Logout requires `POST`. Error responses use JSON, disable caching, and close the HTTP/1 connection. Error output does not include upstream responses or tokens.

| Method | Route                                      | Purpose                                 |
| ------ | ------------------------------------------ | --------------------------------------- |
| GET    | `/auth/login`, `/auth/callback`            | Browser-bound sign-in                   |
| POST   | `/auth/logout`                             | Revoke the current session              |
| GET    | `/api/session`                             | Read the permitted session identity     |
| GET    | `/api/discovery/artists`                   | Search artist names                     |
| GET    | `/api/discovery/releases`                  | Search album titles                     |
| GET    | `/api/discovery/artists/{id}/releases`     | Browse exact release choices            |
| GET    | `/api/discovery/releases/{id}`             | Read full edition data                  |
| POST   | `/api/discovery/releases/{id}/plans`       | Freeze provider-backed edition data     |
| POST   | `/api/discovery/plans`                     | Freeze selected album plans atomically  |
| GET    | `/api/discovery/status`                    | Read provider request state             |
| POST   | `/api/discovery/resume`                    | Acknowledge an unsupported retry pause  |
| GET    | `/api/locations`, `/api/locations/{id}`    | Read locations and coverage             |
| GET    | `/api/locations/{id}/entries`              | Read indexed folder entries             |
| GET    | `/api/settings`, `/api/jobs`               | Read saved settings and jobs            |
| PUT    | `/api/settings`                            | Save an immutable settings version      |
| POST   | `/api/jobs`, `/api/plans`                  | Create a job or prepare an output plan  |
| GET    | `/api/jobs/{id}/plans/{version}`           | Read a saved plan                       |
| GET    | `/api/jobs/{id}/plans/{version}/transfers` | Read paginated transfer records         |
| POST   | `/api/jobs/{id}/plans/{version}/approve`   | Approve the current observation binding |
| GET    | `/api/questions`, `/api/events`            | Read current questions or job events    |
| POST   | `/api/questions/{id}/answer`               | Save a version-bound offered choice     |

List routes use `after` and `limit`, with a maximum of 200 rows. Folder browsing also accepts `folder`, a validated library-relative path. Plan approval and question answers do not start media operations.

`GET /api/events` returns a bounded JSON page. `GET /api/events/stream` sends `job` server-sent events with the same JSON records and durable event IDs. `Last-Event-ID` takes precedence over the initial `after` query on reconnect. The stream keeps at most one page in memory. Four streams can remain open. Closing a stream releases its slot. Idle streams check the session every two seconds. Buffered events also require a current session before delivery. An expired or revoked session produces an `error` event and closes the stream.

For a new snapshot, obtain `GET /api/events/cursor` before loading the paginated read models. Then stream events after that cursor and refresh affected read models. A cursor ahead of the database returns a conflict instead of silently skipping changes. Obtain a fresh cursor and reload the read models in that case. Keep-alive comments are sent every ten seconds. The browser application still needs to implement this recovery flow.

Without a valid production session, the root redirects the browser to sign-in.
With `browser_assets` configured, the root and bundled assets use the same guards
as the API. Without installed assets, it reports the explicit unavailable response.
Explicit development mode uses `_anonymous` instead of Entra and remains loopback-only.

## Connected local Music browser

See [Local album download with developer HTTPS](docs/local-development.md).
The browser uses explicit `_anonymous` development mode and a valid developer
certificate on loopback. It supports exact-release plans, separate source/cover
approval, a single explicit download and verified new-only local publication.
The static design reference remains in [docs/ui-poc](docs/ui-poc/README.md).
This is not complete V1 deployment or live-account acceptance.
