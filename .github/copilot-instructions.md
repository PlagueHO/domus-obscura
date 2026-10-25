# Domus Obscura - Copilot instructions

See AGENTS.md for layout and commands.

## Security

- Keep domain inputs separate from native filesystem paths.
- Accept library-relative identifiers through LibraryPath.
- Reject root escapes, alternate streams, symlinks, and Windows reparse traversal.
- Keep scan iteration and metadata on directory handles. Open every path component separately without following links; never restore checked-path-then-open access.
- Bind configured media and staging roots to native handle identity and ancestry. Require explicit binding after legacy upgrades; never accept a copied owner marker as a replacement root.
- Keep the local file-id patch limited to the borrowed-file high-resolution query. Preserve its provenance and both licenses. Never fall back to short Windows file IDs.
- Keep SQLite on local disk.
- Use parameterized SQL.
- Keep filesystem and network I/O outside database transactions.
- Reject stale job ownership, leases, states, and plan versions.
- Use the returned JobClaim for mutations; never use a worker name as a claim.
- Keep saved settings and plan bodies immutable.
- Recheck indexed observation bindings before approved work.
- Keep incomplete observations distinct from confirmed absence.
- Keep media mutations out of library browsing.
- Require Entra in production. Only explicit development configuration can use the fixed _anonymous identity. Keep development HTTPS, loopback peer/origin checks and command guards. Never infer development mode from missing credentials or failed authentication. Never add a public MCP endpoint.
- Keep the local browser test harness in its separate development example. It can use only fresh temporary databases and generated roots, never the production API router, live providers or existing-library paths.
- Bind login state to one browser and consume it before token exchange.
- Keep pending PKCE secrets and nonces out of SQLite.
- Authorize the signed Entra tenant/object pair, not email or display names.
- Keep runtime private-key operations out of the RSA verification dependency.
- Keep Entra traffic on the fixed HTTPS authority with redirects and retries disabled.
- Validate new TLS material before activation. Never disable client certificate checks.
- Keep bearer credentials out of SQLite, URLs, logs, and serializable DTOs.
- Rotate browser sessions atomically and preserve the old session if creation fails.
- Require the exact Origin and x-domus-csrf header for browser commands.
- Use the guarded TLS listener so connection, parser, and handshake limits stay active.
- Keep blocking SQLite and file work outside async executor threads.
- Keep API preview loopback-only until the public acceptance boundary is complete.
- Commit transfer intent before submission. Never reset an uncertain submission on reclaim.
- Keep peer paths out of filesystem APIs.
- Keep the staging root's OS lock alive through every staged-file operation. Never replace a live owner marker.
- Check permissions on the actual no-follow handle. Require a sealed input receipt after exact stream completion.
- Verify interrupted outputs without rewriting them. Keep partial or different files unchanged and return an error.
- Recheck approvals before new effects. Preserve receipts for already-submitted work when index observations change.
- Keep event replay bounded and check the session before each streamed event.
- Keep secrets, tokens, state, music, and recovery data out of Git.
- Require the durable single-use CatalogPermit for every MusicBrainz HTTP request.
- Use that same catalog budget for each CAA redirect hop. Keep exact listing membership, release, archive item and filename bindings. Never follow arbitrary artwork URLs or HTTP downgrades.
- Keep provider edit approval separate from user approval and rights decisions. Transport hashes do not prove valid images; retain strict normalization.
- Keep its OS lock through response persistence. Use saved intervals and monotonic clocks for provider backoff, not wall-clock deadlines.
- Preserve longer provider backoff. Never automatically clear an unsupported retry pause.
- Bind new acquisition to a validated, frozen provider snapshot. Keep legacy receipts possible.
- Keep multi-album plan writes atomic. Require separate approval for each album.
- Bind artwork approval to the exact JPEG preview and plan version. Never infer user approval from CAA edit approval.
- Require the approved cover before new normalization effects and final handoff. Preserve legacy receipts without inventing approval.

## Purpose

Build a personal home-media application with Rust, React, and SQLite. V1 acquires exact complete-album editions and provides read-only library browsing on Windows and Linux.

## Code Style

- Keep domain policy independent of transport and infrastructure.
- Put orchestration in application use cases.
- Use infrastructure adapters for SQLite and filesystem details.
- Add traits only at real I/O or testing boundaries.
- Prefer the simplest implementation of confirmed scope.
- Return typed errors through domus::Result.
- Record scan failures durably instead of hiding unreadable coverage.
- Keep queues, pages, scan batches, and decoding work bounded.
- Add useful rustdoc for ownership, concurrency, invariants, and recovery.
- Treat the rough 30% teaching-documentation aim as qualitative.
- Do not add filler comments or a numeric documentation gate.

## Naming

| Element                 | Convention | Example          |
| ----------------------- | ---------- | ---------------- |
| Rust modules/functions  | snake_case | scan_location    |
| Rust types              | PascalCase | LibraryPath      |
| React components        | PascalCase | OutputPreview    |
| Database tables/columns | snake_case | scan_directories |
| Persisted job states    | snake_case | backup_pending   |

## Rust and React Patterns

- Keep each business rule in one Rust implementation.
- Generate shared TypeScript contracts with ts-rs.
- Do not edit generated declarations.
- Keep preview rendering distinct from template execution.
- Display filename replacements without changing valid tag text.
- Preserve encoded audio when media processing is added.
- Check every MP3 audio frame for actual 320 kbps and complete decoding. Keep the optional first metadata frame separate; never substitute average or advertised bitrate.
- Independently check FLAC coding boundaries, reconstructed sample ranges and frame CRCs before decoding. Require a supplied MD5 to match; keep an absent MD5 explicitly unverified.
- Write fresh metadata only to empty private staging. Derive tags from the frozen exact release, preserve encoded audio, bind the full input hash and verify the complete written output before saving a receipt.
- Keep JPEG container checks and warning-free native decoding together. Never accept a repaired image or remove the scan, coding, color and allocation limits.
- Use shadcn/ui for application components when that stage begins.
- Do not treat neutral scaffold components as an approved final theme.

## Testing

- Use fresh temporary directories and SQLite databases in workflow tests.
- Keep live credentials and peer downloads out of tests.
- Check required outcomes rather than proxy metrics.
- Distinguish generated-record scale evidence from physical scanning and process-memory evidence.
