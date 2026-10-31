# Domus Obscura - Detailed agent operations

This guide preserves the detailed operational rules from the original root
AGENTS.md. Read the relevant command and service boundaries before changing them.
The root AGENTS.md provides the short command and repository guide.

## Artist photos

Commons is the only installed artist-photo source. Keep its selection separate
from CAA release artwork. An old configuration defaults to disabled.
Require the selected shared file cache before source activation. Passive
configuration reads and validation must not contact a provider or bind a root.

Keep exact MusicBrainz artist UUIDs separate from Commons File titles and native
paths. Never search by name or infer matching identities. Binding commands are
configuration-write operations with browser Origin/CSRF guards. Recheck exact
source, JPEG confirmation and expected binding version after slow work.
Keep removal tombstones so stale saves cannot recreate a removed link.

Require current supported rights before serving even a cached JPEG. Retain
creator, credit, canonical license/source links and supplied notices as plain
text. Keep CC BY-SA outputs under the source license. Do not treat Commons
metadata as a complete personality/privacy/trademark permission grant.
Do not connect artist links to album-cover approval or media mutations.

Commons owns a separate cross-process request lane and durable delay.
Do not shorten timed backoff or clear an unsupported delay automatically.
`POST /api/system/artist-photos/resume` is the explicit guarded recovery command
for an unsupported delay only. It takes the native lock and rejects live work.
Never replace its lock file. Cancellation stops pending submissions, not
completion records for an already submitted request.

Use generated fixtures and local TLS servers for regression coverage.
Live provider downloads and external configuration still need separate approval.

## Backend configuration writers

`ConfigurationService` is the sole access boundary for HTTPS backend JSON.
Keep startup, inspection, validation, and save callers on its typed operations.
Never add direct file reads, parsing, secret merging, or writes in API handlers.
Keep runtime activation separate. Do not use `PreparedServer::load` to validate a
draft; startup can migrate SQLite and bind a cache.

The safest manual editing procedure is to stop the API, edit its JSON file, and
start it again. Preserve private file permissions. Only this operator procedure
can change `system_settings_read_only`; browser requests cannot change it.
Do not restart a service or edit a live external configuration without approval.

If an operator tool edits configuration while the API runs, it must follow the
same writer protocol. For `service.local.json`, the stable sibling lock file is
`.service.local.json.lock`. Open it without following links and hold an exclusive
OS file lock through reading, checking the expected raw-byte SHA-256 revision,
validation, synced sibling-file replacement, directory sync where supported, and
verification. Never unlink or replace this lock file. Keep every file operation
relative to checked native directory handles. Use Rust `File::try_lock`, or an
equivalent native lock: `flock` on Linux and `LockFileEx` on Windows.
The API creates the lock on its first save and retains its identity.
Manual editors that do not implement this protocol must run with the API stopped.
Linux locks are advisory; atomic rename alone does not exclude another writer.
See [Linux flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html),
[Rust File locking](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock),
and [cap-std directory operations](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html).

Backend JSON saves do not activate changes. They do not approve media work, change
immutable plans, create or bind roots, or activate TLS. Validation requires a
current-schema local database and existing valid file/root references. Set up new
cache or staging roots separately. Full startup and operation-time checks remain.
Do not report passive validation as proof that later startup will succeed.

ConfigurationService also owns typed section saves and revision-bound reload
requests. Section updates merge with the latest exact saved revision under the
same writer protocol. Other sections retain their saved values. A browser can
save several changed sections atomically when validation spans section boundaries.

Music Sharing owns only `production_worker.sharing`. Music Acquisition owns the
worker queue and acquisition settings. A section save must preserve the sibling
settings. Sharing is disabled by default. A sharing-only worker needs no staging
root. An explicit sharing account must match the acquisition account when both
are configured; otherwise sharing can reuse the acquisition account.

Folder inspection and explicit native binding are separate from configuration
saves. A save validates policy and existing account references without binding
folders, building catalogs, logging in or opening listeners. The Sharing form
reports saved configuration separately from worker status. Only a fresh worker
heartbeat with the exact saved revision confirms that configuration revision.
An API reload does not restart the worker or prove that sharing is available.

`POST /api/system/configuration/reload` explicitly replaces the API host with
the expected saved revision. The runtime prepares the replacement before listener
handover. It retains the old bound socket for restoration. Preparation failure
keeps the previous host active. A failed restoration is a fatal host error and
requires operator recovery; it is not reported as successful activation.
Read-only policy and browser command guards apply. Host-owned local operations
must be idle and remain excluded during preparation. Reload never interrupts a
receive or publication and never restarts separate workers. Review each worker's
configuration and restart it separately at a safe boundary.

Read/validate operations remain passive. Only explicit startup or reload can
initialize host resources, load identity, or activate a listener/TLS generation.
Origin changes can require manual browser navigation. Identity changes can
require login. Check the active revision after reconnecting. A pending command
or lost response is not evidence of activation. Never retry reload automatically.

After a stale revision or failed save, retain the draft and review the saved
file. A pre-replacement error leaves the previous file intact. A post-replacement
sync or verification error reports an uncertain saved result, not rollback.
Refresh saved values before retrying. An interrupted save can leave an owned `.pending` file;
inspect and remove only that exact temporary file while the API is stopped.
Never delete the configuration or lock file as automatic recovery.

## Indexed Library

Keep browsing passive. Explicit full scans and direct-folder refreshes use the
same durable scan lane, location target and fenced scan claims. Resume only the
same scope. A direct-folder refresh lists child directories but never scans them,
marks descendants absent, or renews whole-location coverage.

Metadata inspection reads MP3 and FLAC through no-follow native handles. Keep the
8 MiB metadata envelope, 1024 frame/comment limit, 64 KiB MPEG probe, 64 KiB saved
tag body and 128-entry persistence batches. Unsupported formats and failed tags
remain indexed files with diagnostics. Do not reuse browsing header evidence as
full-stream validation or exact-edition proof. Migration leaves legacy metadata
unobserved until an explicit scan.

Artists, albums, members and assets use location-scoped keyset pages. Conservative
matches accept at most 24 queries for one location and return at most 50 folders
per album. Compare stored normalized keys, not filenames, fuzzy aliases or live
provider results. Keep matching and acquisition decisions separate.

Local JPEG preview requires the indexed location/file binding, unchanged native
identity, size and high-resolution modification evidence. Reuse strict bounded
image processing; never serve original bytes or open arbitrary asset paths.
Do not extract embedded covers or infer artwork approval from a preview.

## Commands

`domus worker <absolute-configuration-file>` is the separate foreground production
worker. Its explicit `production_worker` section is distinct from development-only
`local_acquisition`; never relax production Entra or loopback API guards to start
it. Load through ConfigurationService without starting an API host or migrating
SQLite. Keep paths and credentials in host configuration, not operation requests.

The durable queue accepts offline submissions. Validate capacity from 1 through
1024, default 64. Admission, duplicate checks, and capacity checks are atomic.
Failed/waiting requests retain capacity until explicit retry or dismissal.
Persist the provider/acquisition configuration digest with immutable requests.
Configuration save or API reload never activates a new worker configuration.

Keep one acquisition pipeline and one independent scan. The native process lock
excludes another daemon in the same state directory. Account locks also cover
the existing worker/search/folder entry points in that deployment. Hold all
native ownership through thread/session draining. Never unlink the stable lock
files or run the same account in another deployment.

Keep returned JobClaim and ScanClaim authority separate from request ownership.
Renew request leases while operations run. Stop new effects at safe boundaries,
not by aborting a blocking thread. Preserve active receive/copy receipts and an
incomplete scan frontier. Forced-exit recovery waits for fenced lease expiry.
Do not finalize interrupted coverage as confirmed absence.

Only interrupted accepted requests recover automatically. Failed work and saved
questions need explicit action. Resolving a decision and applying recovery still
do not submit work; retry the saved request separately. Dismissal releases only
an idle failed/waiting worker request, never album state or media. Publication
continues to reject user cancellation. Never convert uncertain submissions into
fresh queue commands. Preserve the existing finite per-operation reconnect budget.

See the [production worker guide](../README.md#separate-production-worker).
Native service installation and reboot acceptance remain issues #17 and #18.
Tokio documents that started [blocking tasks cannot be aborted](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html);
the long-lived scheduler and its bounded operation lanes use dedicated threads.

Use the pinned tools from rust-toolchain.toml, .node-version, and package.json.

On Windows, use the launcher. It finds a complete installed MSVC environment. It accepts the installed stable Rust alias only when its exact version matches the pin.

```powershell
.\dev.ps1 bootstrap
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
```

Use `.\dev.ps1 local-test` for the isolated sample-data browser harness at
`http://127.0.0.1:4173`. It needs no provider credentials. Use
`.\dev.ps1 local-test-build` to build it without starting a listener.
The harness is a separate development example, not a production auth mode.
It must never accept existing library roots or databases.
The real HTTPS API also supports explicit `environment: development` configuration.
It uses the fixed `_anonymous` identity without Entra and accepts a valid developer
certificate. Keep it loopback-only. Production is the default and requires Entra;
missing credentials must never select development mode.

`domus worker-once <absolute-configuration-file>` is an experimental blocking
worker for an approved acquisition with an explicitly selected source. It opens only outgoing
connections and needs explicit plaintext Soulseek login acknowledgement.
Never run it against a live account without separate approval. Its optional
playback export stays inside private staging and leaves the job in `publishing`.
Its optional `source` configuration maps every frozen release track to one
user-selected peer before network access. Require explicit edition/permitted-use
acknowledgement. Do not treat that acknowledgement as independent edition proof.
Never replace a committed source or retry uncertain submissions. The worker can
make at most three server reconnect attempts with 1-, 2- and 4-second backoff.
Reconnect does not reset that budget or resend `QueueUpload`. Reconcile sealed
local inputs and retained exact-peer queue/offer evidence before further effects.
Keep incomplete files unchanged and require operator review when evidence cannot
resolve uncertainty.
It is not production publication, peer search or a complete setup workflow.

`domus publish-new-once <absolute-configuration-file>` remains a separate,
explicit local new-only publication command. It requires a current approved
plan, completed normalization, the private staging lock and new-only
acknowledgement. It cannot grant replacement permission. The connected API
requires a separate decision for each existing destination before acquisition.
Commit all destination/file intents before library effects. The default action
creates only new album folders. Never adopt an unbound existing folder,
overwrite a partial file or recreate a missing receipted file. Preserve staging until every required destination verifies. Preserve all failed copies. Reclaim can verify exact synced files without another
download or normalization. A verified primary with an unavailable backup
remains `backup_pending`. Recheck successful destinations, with the primary
last, before completion. `publication-status` and the protected publication GET
route are read-only.

The connected API exposes a bounded preview of the existing files and planned
outputs. Each saved decision binds the approved plan version, native root and
root identity, destination folder identity, exact file hashes and action.
`skip` cancels the queued album request; `copy` selects a deterministic
separate destination; `replace` retains the exact reviewed folder before
publishing the complete new album. Never infer replacement permission from
source, cover, new-only or incomplete-index acknowledgements. Re-open and
recheck the exact review immediately before each effect. Changed files or root
identity are conflicts. An offline backup remains pending; when it returns,
apply its saved decision only if its review is still exact.

The command copies each album into a hidden `.domus-pending-*` sibling folder.
It verifies every file before it atomically renames the complete folder to its
final name without replacing an item that appeared at the destination. A
replacement first moves the complete reviewed original directory to its
`.domus-retained-*` sibling with a handle-relative no-replace rename. The
publication and replacement journal preserve each transition across restart.
Library scans, direct refresh and sharing exclude pending, retained and
`.domus-restore-*` folders. Never merge tracks or adopt extras into the new
album.

After every destination completes, the connected API offers explicit restore
of a retained original, with a separate preview and confirmation for each
destination. The GET on
`/api/jobs/{id}/plans/{version}/publication/replacement/{location}/restore`
reads and hashes both versions through checked native handles. A failed preview
returns an explicit unverified result; its saved file lists are not current
evidence. Preview does not approve or move files.

The POST on the same route requires `expected_preview_sha256` for new restores,
in addition to the saved review and publication binding. It repeats verification
under the visibility lock before saving immutable approval and journal intent.
Changed current files and supported flat-folder extras can be preserved after a
fresh preview and approval. The retained original must still match its exact
saved review. Missing folders, unsafe entries, changed roots, or changed retained
originals block restore. Never accept a client-supplied replacement manifest.

Restore uses the same visibility lock and a durable multi-step journal. It moves
the current folder to a
hidden `.domus-restore-*` sibling, restores the exact reviewed original, then
retains the approved displaced current version. On retry, inspect and hash all possible
folders before advancing a state. Never overwrite, delete or infer approval
when a folder identity, manifest or saved transition differs. The API does not
offer restoration while any destination remains pending. Once intent is saved,
continuation cannot approve changed files. Legacy interrupted restores remain
bound to their original publication and replacement evidence; they do not gain
permission to preserve different files.

Restore errors and recovery questions are saved per destination and remain in
Decisions across reloads. Saving an answer does not approve files or continue
restore. Completed acquisition state and historical publication receipts remain
unchanged. Inspect the restore journal to determine which version is visible.
After a lost command response, read status and prepare a fresh preview before
an explicit continuation. Repeated completed commands verify the saved result
without another rename. Destinations complete independently; there is no
cross-drive atomicity guarantee. These safeguards do not constitute power-loss
acceptance. Recovery inventory and deletion use the separate workflow below.
Restoring a later displaced version remains separate work.
The sharing boundary hides incomplete publication destinations. Publication
holds the stable native exclusive visibility lock before journal intent and
media changes. Browse responses and uploads hold the matching shared lock.
Lock admission is nonblocking. A busy operation must retry at its normal
boundary, not wait indefinitely. Retained destination intents keep failed
copies hidden after restart. A verified primary does not need a backup receipt
to become eligible. Catalog generation and native file evidence are rechecked
before reads. These advisory locks do not prevent external programs from
changing files on Linux.
Replacement and restoration acquire this same visibility lock before durable
intent and filesystem effects. Keep retained and temporary paths out of scans,
sharing and direct refresh. Do not remove recovery folders or invalidate their
journal as part of ordinary publication.
Do not run it against existing library roots without separate approval.

### Scratch cleanup and retained-version deletion

Publication admits scratch cleanup only with successful completion at every
required destination. Save the root binding and admission with the completed job
transaction. Never reuse the released publication `JobClaim` for cleanup. The
cleanup journal supplies separate authority for that exact run and manifest.
Close publication readers before cleanup, and keep the staging OS lock alive.

Never clean backup-pending retry input, unsealed partial transfers, failed
destination copies, older failed normalization attempts, approved evidence,
playback exports, recovery folders or unrelated files. Keep `owner.lock` and
immutable database receipts. A cleanup error does not invalidate a verified
published album. Record it separately and require explicit retry.

Recovery inventory is a bounded journal read, not a recursive disk scan.
Recorded byte sizes are not current allocation measurements. Unknown, changed,
missing and offline evidence must remain explicit. A known folder prefix is not
proof of ownership. Legacy data without exact provenance cannot gain deletion
authority through migration.

Retained deletion needs a separate fresh preview and irreversible confirmation.
Bind approval to the exact version, plan, destination, root/directory identities,
file identities and hashes, current album and journal state. Keep original and
restore-displaced versions distinct even if a folder name is reused. Keep
Library browsing and MCP read-only.

Acquire the publication visibility lock before deletion intent. The same lock
excludes restore. Once deletion intent exists, reject restore preview and commands
for that retained version. Keep historical replacement and restore receipts
unchanged. Check every remaining item before continuation; never approve changed
files as part of a retry.

Commit each file's removal intent before unlink. An interrupted intent can
reconcile confirmed absence, but cannot prove which process removed the bytes.
Missing files before intent are errors. Remove only listed checked files and
then the exact empty owned directory. Do not use recursive native-path deletion.
Persist partial outcomes and errors. A failed final receipt is not a rollback.

API recovery operations run outside async executor threads and keep their media
permit until blocking work ends, even if the request times out. API reload and
shutdown must not release that protection early. Linux external writers must
cooperate with the visibility lock; do not claim protection against arbitrary
external mutation or untested physical power loss.

`domus job-control <local-database> <job-id> <plan-version> <pause|cancel>`
records a version-bound operator request. Settle it only at a safe file/transfer
boundary with a live fenced claim, or atomically when no worker lease is live.
Never stop uncertain submissions or active receives. Block new submissions and
publication handoff while the request is pending, but preserve in-flight receipts.
Reject publication, backup-pending and terminal states.
Pause questions use the normal answer command. `job-control-continue` applies
only that saved answer after rechecking the current plan and observations.
It does not claim, approve, fetch, open staging or start work. Keep operator
controls separate from artwork questions. Retain all staging and receipts.
`job-control-status` is read-only. Control commands must not migrate old state.

`domus worker-check <absolute-configuration-file>` checks local inputs through
a current-schema read-only SQLite snapshot. It must not claim jobs, record sources,
migrate data, open staging or contact a network. A successful check is not
authority to skip the worker's live claim, staging or approval checks.

`domus artwork-local-prepare` accepts an explicitly permitted local cover without
CAA. Preserve its content-based local provenance. Never label it as provider
evidence or infer preview approval from the permitted-use acknowledgement.
`domus artwork-approve` still requires the exact normalized JPEG preview hash.
`domus artwork-preview` writes those retained JPEG bytes to standard output
through a read-only snapshot. Keep binary output separate from error text.
Use binary-safe redirection and verify the hash before showing the preview.

Use `domus artwork-caa-prepare` only with fresh verified CAA cache evidence and
explicit permitted-use acknowledgement. It must atomically freeze the listing,
image hash, source chains and fetch times with the normalized candidate.
Keep cache eviction separate from immutable approved evidence. Recheck retained
evidence during preview, approval and normalization reads. Legacy and local
candidates have no CAA evidence; do not manufacture it during migration.
`domus artwork-evidence` reads the bounded summary without network access.
The API's CAA preparation route is cache-only and retains normal command guards.

Use the `_or_ask` artwork preparation adapters for CLI/API commands. Typed
recoverable image issues must persist a plan-bound artwork question. Never turn
corruption, stale state, storage or network failures into a recoverable decision.
`artwork-check` can pause an idle job; `worker-check` must remain read-only.
An active worker must use its fenced claim and reach a safe transfer boundary.
Never pause or resume submitting/active transfers as artwork recovery.
Reuse the current unanswered question and preserve its safe paused state.
Artwork answers require a separately approved exact JPEG and freeze its candidate
and hash. `artwork-resume` rechecks the approved plan, destinations, preview and
transfer state before restoring only that state. It must not start media work.
Keep answers, resume and worker claims separate. Preserve all transfer receipts.
Fully received inputs may continue through `worker-once` without a new Soulseek
connection. Never use that local-only path for planned or uncertain inputs.

`domus catalog-release <local-database> <release-id>` loads an exact MusicBrainz
release through the normal durable provider budget and cache. It creates no
job, plan or approval.
MusicBrainz and CAA share one durable network permit, including each CAA redirect.
Each configured service host admits at most four running or waiting operations
across both adapters. Cache access is separately bounded; a valid hit needs no
network slot. One operation can spend at most 15 seconds waiting for admission,
including mandatory permit spacing and all CAA hops. Existing transport deadlines
remain separate. Full capacity, longer backoff and manual pauses return an
explicit error. Retrying admission before any network effect is not permission
to retry a failed request.

Provider headers save the longest `Retry-After` before body processing. Preserve
that interval through timeout, cancellation, body/cache failure and owner exit.
The permit also retains observed headers so completion cannot shorten the delay
if the initial header write fails. The persistence error remains visible.
Keep the native lock through completion. Do not clear an unsupported interval
automatically. Explicit pause acknowledgment can recover an abandoned owner only
after acquiring its native lock. Saved intervals, not wall deadlines, govern
restart and recovery. The gate coordinates one local database; separate
installations behind the same public IP are not coordinated.

`GET /api/discovery/status` is passive. `known_retry_after_seconds` is null for
a live owner or manual pause. The legacy `retry_after_seconds` field is only a
recheck hint for a live owner, not its completion time. Provider errors expose
`provider_state` for active, timed-backoff, paused and full-capacity conditions.
The browser shares one bounded status observer while metadata is pending. It
does not retry the metadata request when timing expires. Use the original
metadata action for explicit retry; never substitute a worker/acquisition retry
or approval. A disconnected browser cancels pending submission, but started
response persistence must drain. Do not admit replacement work until its slot
is released.

The isolated browser harness has `/?provider=1` presentation fixtures for these
states. They make no provider, acquisition or approval calls. Process-exit and
simulated timing fixtures do not prove physical process suspension or host-sleep
acceptance. Record unperformed physical and live-provider checks separately.

`domus catalog-plan-prepare <local-database> <release-id> <request-json>` builds
the normal provider-backed plan from the fresh cache without network access.
Its bounded request has the same fields as CatalogPlanRequest. It cannot
override provider track text, approve the result or start acquisition.

The retained catalog and CAA preparation commands also accept
`--host <absolute-host-configuration-file>` in place of `<local-database>`.
`source-search` and `source-folder` accept the same option before their existing
source configuration path. Use the read-service host format (`database`,
`services`, optional `cache`), not HTTPS startup configuration.

Configured CLI access requires a current-schema database and, when selected,
an already-bound shared cache. It never initializes a root, changes a binding,
repairs an owner marker or removes pending cache files during loading. Keep
explicit host startup separate. Do not use an initializing cache open for reads.
Protect all host, request, source and credential files from media/cache overlap.

Both source configuration files must identify the same native database and
permit Soulseek. Reject disagreement before credentials or network access.
Keep source metadata loading read-only. File-cache reads must use a connection
without a long-lived SQL snapshot; no file-cache I/O belongs in a SQL transaction.

Omitting host `cache` selects legacy SQLite storage. Selecting a file cache never
copies, migrates or falls back to SQLite entries. Only `catalog-release` can fetch
on a cache miss, through `ServiceHost` and the existing provider budget.
Plan preparation, CAA preparation and source metadata loading remain cache-only.
Treat corrupt entries and storage failures as explicit errors, not misses.
Preserve cooldown and unsupported retry pauses. Existing command forms retain
their behavior. Host selection cannot change approved jobs or replace immutable
plan, JPEG, source-review or receipt evidence.

`domus source-search <absolute-configuration-file>` performs one bounded
metadata-only Soulseek search for a fresh cached exact release. Require separate
live-account approval and plaintext-login acknowledgement. It opens outgoing
server-mediated `P` connections only, with no listener, shares or audio download.
Private results are excluded. Search results do not prove complete folder
contents, actual encoding, exact edition or permission to download.
Never turn a search result into source or artwork approval.

`domus source-folder <absolute-configuration-file>` reads one selected peer
folder through an outgoing direct connection and assesses its complete bounded
listing against the fresh cached release. It uses the same separate live-account
approval and plaintext-login acknowledgement as source-search. Keep peer folder
identifiers out of native filesystem APIs. Reject wrong tokens/folders, escapes,
duplicate paths, partial compressed data and excess counts instead of truncating.
Keep exact name/number matching, advertised quality and actual audio/edition
verification separate. A complete name mapping still requires user review.
Use `audio_profile` with the matching `format` to assess a supported profile.
Without that field, MP3 still means 320 CBR. New plans freeze an audio-policy
version and exact profile; source review and normalization must use that binding.
Other accepted profile matches need a new plan and approval, never automatic
fallback. Missing advertised bitrate or VBR flags remain unknown.
Never turn this report into a source approval, job claim, download or retry.

`domus source-candidate-prepare` imports only a bounded local listing object,
not the complete folder report. It saves an immutable review against one exact
approved plan. Keep preparation, digest-bound edition/permitted-use approval and
worker consumption separate. Approved listings do not prove edition, rights or
actual encoding. Keep incomplete and ambiguous mappings unapproved.
The worker can use one approved `source_candidate` instead of a manual `source`;
never accept both, fall back between them or replace a committed source.
Recheck observations and candidate approval under the live claim, then commit
its review binding with all transfer intents. Preserve manual/legacy receipts
without manufacturing reviews. Startup and candidate commands require the
current schema; never migrate state as a side effect of worker loading.

Before any investigation, design, implementation or change to an external-service
client or service, read the current API or protocol documentation for the affected
operations. Do not rely on internal knowledge, assumptions or existing code alone.
Check request and response fields, optional metadata, limits, errors and documented
client/version differences. Cite the relevant documentation in the explanation
or directly related repository documentation. If documentation is missing or
unclear, state that limit and check maintained reference clients or isolated
protocol fixtures before drawing conclusions. Do not present assumptions as facts.
For Soulseek client or service work, always start with the
[Soulseek protocol documentation maintained by Nicotine+](https://nicotine-plus.org/doc/SLSKPROTOCOL.html).
It is a community reference, not an official specification; keep that distinction
clear and check maintained client implementations when necessary.

The Rust Soulseek client must support later extension to common client
operations. Follow the "Soulseek client extension contract" in README.md.
Keep wire codecs independent of sockets, SQLite, catalogs, worker state and
filesystem paths. Keep operation modules separate from application approvals.
The session-owned nonblocking I/O loop supplies one reader and one controlled
write path per framed connection. Keep its bounded typed routing and
session/connection/token correlation. Public operations still use one borrowed
peer channel; this is not a concurrent search/chat API. Do not add a competing
reader or operation-local message-skipping loop. Keep raw file reception outside
the framed loop with a single ownership handoff.
Do not enable uploads, shares, listeners, port mapping or transfer retries
as a side effect of extension work. Preserve uncertainty and approval checks.
The append-only transfer evidence journal is not permission to submit again.
A queue reply has no request token; never treat silence or position zero as
confirmed absence. Reconciliation may commit a verified sealed input receipt
after observations change, but new wire or media effects retain current approval
checks. Keep filesystem and network I/O outside evidence transactions.

Core services are in-process Rust modules, not separate servers.
Keep discovery, metadata, artwork retrieval and acquisition behind the contracts
in `application::services`. Register installed adapters in `ServiceHost`.
Give every remote API integration a dedicated client struct behind an
application-owned trait at the I/O boundary. Reuse an existing provider trait
when it fits; do not add a redundant abstraction. Keep requests, protocol
details, parsing and transport limits inside the adapter, not UI or route
handlers. Keep workflow policy outside the client. Alternate providers and
test clients must be replaceable through host registration without caller changes.
Use shared cache functionality and preserve provider budgets.
Use provider-scoped IDs. Never treat another provider's ID as a MusicBrainz ID
or exact edition proof. Keep provider failures and independent pagination visible.
Do not add hidden fallback, automatic identity mapping or acquisition retries.
Preserve independent service selections and fail unavailable providers before
credentials, claims or network access.

`domus service-call <absolute-host-configuration-file> <absolute-request-file>`
uses the same typed read-service commands as the local MCP adapter.
It can update provider caches and budgets, but cannot approve or acquire media.
`artwork_fetch` may cache a listing-bound original image. Return its digest and
byte count, never raw image bytes or an approval claim in the tool result.
Tool arguments must not select credentials, database paths or native destinations.
Artwork bytes remain internal input to the existing normalization and approval
flow. Acquisition remains a separate approved-job operation. Do not implement an
MCP listener or expose mutating tools as a side effect of adding a provider.

`domus mcp-stdio <absolute-host-configuration-file>` is a private stdio-only
adapter for per-request MCP revision 2026-07-28 and initialization-based
2025-11-25 compatibility. Validate modern version and client capabilities on
every request. Self-reported client identity is not application authority.
Never inherit modern capabilities or readiness from the legacy handshake.
Keep its four tools read-only at the
application boundary. Cache and budget updates remain normal provider effects.
Never add a listener, approval, worker start or acquisition tool implicitly.
Keep input, pending messages and output bounded. Keep stdout JSON-RPC-only.
Cancellation suppresses the response, but must not abandon current provider
budget/cache persistence. A transport error must drain that operation before
returning failure. Startup must not create or upgrade a database.

On Linux, use the same Rust task interface.

```sh
cargo task bootstrap
cargo task check
cargo task build
cargo task smoke
```

Use a targeted Rust test during development.

```sh
cargo task test interrupted_scan
```

Regenerate shared TypeScript declarations after a Rust contract change.

```sh
cargo task contracts
```

Run the separate scale check after index/query changes.

```sh
cargo task scale
```

Use the Windows launcher in place of the Cargo task prefix on Windows.

The connected local Music browser is in `frontend/app`. `build` also builds it
to `frontend/dist-app`. See `docs/local-development.md` for explicit anonymous
HTTPS setup with a developer certificate. `scripts/local-setup.ps1` creates only
a fresh Windows setup and never changes certificate trust or enables an account.
Keep its optional `local_acquisition` development-only. The guarded run command
uses only host-configured credentials/staging and approved source/cover/plan
bindings. Hold the same fenced claim and staging lock through new-only
publication. Never add automatic retries or overwrite existing album content.
`settings-default` exports the authoritative defaults. `staging-init` creates
and binds private staging through its normal ownership rules.

## Layout

- crates/domus/src/domain: path, job-state, output-template, and edition-plan rules.
- crates/domus/src/application.rs: read-only indexing use cases.
- crates/domus/src/infrastructure: SQLite, filesystem, Entra, direct TLS, and guarded API adapters.
- crates/domus/migrations: versioned schema.
- crates/domus/tests: isolated filesystem/database workflows and scale checks.
- frontend/src: neutral React components and generated Rust contracts.
- frontend/local-test: isolated sample-data browser harness, not the production application.
- crates/domus/examples/local_test.rs: loopback-only test server using temporary fixture state.
- xtask: shared development tasks.
- PRODUCT.md: approved product context and open design decisions.

## Changing a shared rule - Checklist

1. Change the authoritative Rust rule.
2. Add an isolated regression test.
3. Regenerate affected TypeScript declarations.
4. Run the targeted check.
5. Run the full check before delivery.
6. Update directly related documentation.

## CI Pipeline

- **Check and build** checks generated contracts, formatting, Clippy, Rust tests, frontend types/tests, and builds on Windows/Linux.
- **Million-record pagination** runs separately on both systems.
- Treat configured CI as unverified until a remote run succeeds.

## Conventions

| Concern                  | Rule                                   |
| ------------------------ | -------------------------------------- |
| Rust formatting          | rustfmt                                |
| Frontend/docs formatting | Prettier                               |
| Line endings             | LF with a final newline                |
| Lockfiles                | Keep Cargo.lock and one pnpm-lock.yaml |

## Operational boundaries

- Use generated or permitted fixtures for local checks.
- Keep application state outside configured library roots.
- Get separate approval for live downloads, existing-library writes, external configuration, software installation, and public exposure.
- Do not deploy or call this foundation the complete V1 application.
- Preserve user-authored guidance.
- Do not modify the previous music-organizer repository.
