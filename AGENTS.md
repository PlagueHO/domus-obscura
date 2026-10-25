# Domus Obscura - Agent instructions

## Commands

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
Never replace a committed source or retry uncertain submissions.
It is not production publication, peer search or a complete setup workflow.

`domus publish-new-once <absolute-configuration-file>` is a separate, explicit
local publication command. It requires a current approved plan, completed
normalization, the private staging lock and new-only acknowledgement.
Commit all destination/file intents before library effects. Create only new
album folders. Never adopt an unbound existing folder, overwrite a partial file
or recreate a missing receipted file. Preserve staging and all failed copies.
Reclaim can verify exact synced files without another download or normalization.
A verified primary with an unavailable backup remains `backup_pending`.
Recheck successful destinations, with the primary last, before completion.
`publication-status` and the protected publication GET route are read-only.
This command exposes partially copied final folders. It is not an atomic album
switch, replacement, restore, cleanup or power-loss acceptance.
Do not run it against existing library roots without separate approval.

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
`domus catalog-plan-prepare <local-database> <release-id> <request-json>` builds
the normal provider-backed plan from the fresh cache without network access.
Its bounded request has the same fields as CatalogPlanRequest. It cannot
override provider track text, approve the result or start acquisition.

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

The Rust Soulseek client must support later extension to common client
operations. Follow the "Soulseek client extension contract" in README.md.
Keep wire codecs independent of sockets, SQLite, catalogs, worker state and
filesystem paths. Keep operation modules separate from application approvals.
The current blocking, single-peer adapter is not a concurrent dispatcher.
Before concurrent search/browse/chat/transfer support, add one reader per
connection with bounded typed routing and session/token correlation.
Do not copy operation-local message-skipping loops into a persistent client.
Do not enable uploads, shares, listeners, port mapping or transfer retries
as a side effect of extension work. Preserve uncertainty and approval checks.

Core services are in-process Rust modules, not separate servers.
Keep discovery, metadata, artwork retrieval and acquisition behind the contracts
in `application::services`. Register installed adapters in `ServiceHost`.
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
