# Product

## Delivery tracking

Issue #1 remains the approved V1 scope tracker. Its linked checklist separates
remaining delivery tasks, unresolved decisions and explicitly future work.
Merging the foundation does not complete or close that tracker. Future tag rules,
conversational integration and cross-core design are not V1 acceptance gates.
Use the issue templates for new work and preserve the confirmed requirements
below when closing individual delivery tasks.

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

One configured user manages a personal music library from desktop and narrow-screen browsers. The library has more than 500,000 files across a main location and backup locations.

## Product Purpose

Domus Obscura makes complete-album acquisition clear, safe, and recoverable. The primary workflow finishes when the main library files are verified. Backup completion is tracked separately.

## Operating Context

Run the API and worker as separate Rust processes on the same Windows or Linux host. Keep shared SQLite state on local disk. Use manual installation and supervised services. Node.js is a build tool, not a production runtime.

Use Entra ID for one tenant/user pair in production. Explicit local development uses the fixed `_anonymous` identity without authentication and permits a valid developer certificate. Keep this mode limited to direct loopback HTTPS access. Missing production credentials must fail, not enable anonymous access. Serve the app through direct HTTPS. Use MusicBrainz and Cover Art Archive for release metadata and artwork. Use Soulseek for permitted complete-album transfers and explicitly configured read-only sharing.

## Capabilities and Constraints

- Let the user choose the exact release and audio format for each album.
- Require a complete album from one peer.
- Default to MP3 320 kbps constant bitrate; allow FLAC.
- Preserve encoded audio without transcoding.
- Show names, tags, artwork, collisions, and replacement actions before approval.
- Retain replaced files until explicit recovery deletion.
- Require an explicit state-bound decision for restoration.
- Browse main and backup libraries without modifying media.
- Keep unknown coverage, offline observations, and pending backups visible.
- Keep questions and notifications in a persistent in-app inbox.
- Share Rust policy across worker, API, CLI, and local MCP adapters.
- Exclude playback, streaming, Plex integration, bulk cleanup, missing-track repair, multi-peer assembly, and runtime AI from V1.

## Connected Acquisition Recovery

The connected application has a paginated **Decisions** inbox. It retains pending
answers and saved answers whose jobs still await a recovery action. Question
details expose the actual artwork or pause-control relationship and current plan
state; prompt text is never used to select a recovery command. Saving an offered
answer and applying it are separate explicit actions. Both use the existing
version-bound, guarded backend commands. Neither starts a worker or a transfer.
Obsolete decisions are view-only.

A safely paused, current, unanswered artwork question can use the request's
normal exact-JPEG preparation and approval interface. Answering requires the
approved cover; recovery rechecks its saved binding and transfer safety. Source
changes stay locked while paused. Generic questions without a supported browser
recovery action remain operator-controlled. A separate foreground production
worker now accepts bounded durable requests, with one acquisition pipeline and
one independent scan. Queue capacity is operator-controlled; requests can be
accepted offline. Interrupted requests recover from saved evidence. Failed work
and saved questions require explicit action. Source discovery remains
development-only. Native supervised service packaging and complete Windows/Linux
acceptance remain open.

## System Configuration Form

The System configuration form views and updates supported backend JSON settings
through typed controls.
Artwork size, accepted audio formats, preferred formats, quality priorities,
output templates and future tag rules are system-wide library policy, not
per-user preferences. All users share the same library. Only the system
administrator controls this policy. Do not add user-specific copies or overrides
of these settings.

The current production host authorizes one configured Entra tenant/object pair.
That identity controls administration; there is no general multi-user role model
yet. Explicit anonymous development access remains loopback-only and is not a
production administrator role. Before adding other production users, enforce
administrator-only system-setting writes in the backend. The read-only switch
is an additional operator restriction, not a replacement for authorization.

Cover all supported backend JSON configuration fields through typed form controls,
including optional and provider-specific settings. Show field descriptions,
validation errors and whether a change needs a process restart. Distinguish saved
configuration from the configuration currently active in each process. Never show
a saved change as active before it takes effect.

A backend JSON setting must control whether the form is editable or read-only.
Read-only mode must still allow safe configuration viewing. Enforce this mode in
the backend on every configuration write; disabled UI controls alone are not
protection. Do not allow the UI to change this control setting or bypass it through
another endpoint. The operator must change it outside the UI. The exact setting
name is `system_settings_read_only`. It defaults to `true`. The operator must
change it in the server JSON file. Versioned shared-library output policy remains
stored in SQLite. Backend JSON has a separate revision-bound save operation.
System settings is a route inside the main application, with its normal header
and navigation. It also supports explicit registration of existing server-side
main/backup library folders and separate read-only index scans. Both commands
require the same backend edit policy and browser command guards. Registration
uses normal native root identity, ancestry and overlap checks. It cannot create
folders, move media, replace existing roots or rebind legacy roots.

The system output-settings API uses the existing immutable version history and rejects
stale versions. Existing plans keep their saved settings. Backend JSON changes
remain inactive until an explicit API-host reload or operator restart.

`ConfigurationService` owns backend JSON loading, inspection, validation, secret
merging, revisions, and persistence. Startup and API handlers use that service.
`GET /api/system/configuration` returns its immutable active startup snapshot and
a bounded, no-follow read of saved JSON. Secret-file paths and Entra identifiers
appear only as configured states. Both raw JSON revisions are hashed.

The typed editor covers the host, provider selections, optional cache and browser
assets, Entra references, TLS references, and optional development acquisition.
`POST /api/system/configuration/validate` checks a draft without saving it.
`POST /api/system/configuration/save` repeats validation and atomically saves it.
Normal authentication and development loopback/origin guards apply. Commands
require the exact Origin and CSRF header. A host without a configuration service
returns an explicit unavailable error.

Saved JSON remains inactive until reload or restart. Changed external JSON is unvalidated.
An exact revision saved through the service is passively validated, but startup
checks remain required. Validation can read local files, verify TLS, and query a
current-schema database. It does not migrate SQLite, create or bind roots, contact
providers, activate TLS, or start work. Referenced inputs can change after a check.
New cache or staging roots need separate operator setup before browser saves.

Saves use a stable configuration lock and reject stale revisions. All operator
writers must use the same lock protocol, or stop the API before editing.
Unrestricted editors that ignore the lock are outside this guarantee.
Failed saves retain the browser draft. Pre-replacement failures preserve the
previous file. An error after replacement explicitly requires reloading and
reviewing the saved file; it does not claim a rollback.

Do not return saved passwords, tokens, private keys or other secret values to the
browser. Show only their configured state. Where secret updates are supported,
use write-only replacement controls; an unchanged or blank field must not erase
an existing secret. Keep secret values out of logs, URLs and audit records.

Validate proposed changes with the same authoritative Rust rules used at startup.
Save changes atomically and reject stale edits instead of overwriting concurrent
operator changes. Report failed saves and activation failures explicitly. Preserve
the last valid configuration when validation or persistence fails. Record which
settings changed without recording secrets.

Keep production authentication, guarded browser commands, TLS validation, native
root binding and other operational boundaries in force. A configuration save must
not implicitly approve media work, rebind roots or change frozen plans, saved
settings versions, approvals or receipts. Identify restart-only changes and
changes that need a separate explicit operation.

Acceptance must cover complete supported-field coverage, valid and invalid edits,
secret redaction and replacement, stale-edit rejection, failed-save recovery and
restart-required status. Verify that read-only mode rejects direct write requests
as well as disabling form edits, and cannot be disabled from the UI.

System configuration uses a sidebar within the normal application shell:
**Core application** contains General, Identity, TLS, and Storage/cache.
**Media > Music** contains Providers, Acquisition, Output policy, and Library
roots. Other media categories can be added later; no Movies page is implemented.
The JSON storage format remains unchanged.

Open each section directly to its saved settings form. Use the shared Component
library and a compact reload notice only when saved backend changes need
activation. Keep running values, revision identifiers, and optional validation
under Advanced. Do not present a configuration-state dashboard as the main task.
Use a section picker on narrow screens. Discarding one section must retain drafts
in other sections.

Typed section commands use `POST /api/system/configuration/sections/validate`
and `/sections/save`. They submit only selected changed sections. The service
merges them with saved JSON under the existing writer lock and validates the
complete candidate. Batched changes are atomic. A stale whole-file revision
still fails, even when another writer changed a different section.
Successful section saves retain drafts in other sections. Navigation keeps
backend and Music policy drafts. Music output saves retain their existing
immediate effect on new plans; existing plans retain frozen policy versions.
Library registration and scanning remain separate explicit operations.

**Refresh saved values** is passive. **Reload** sends a revision-bound command
through ConfigurationService to replace the API host with saved backend JSON.
It does not restart separate workers. The service reports pending, active, or
failed reload state, separately from save/validation state. The runtime prepares
the replacement host before listener handover and keeps the old listener bound
for recovery. Preparation failure leaves the old host active. Busy host-owned
media operations reject reload instead of interrupting effects or receipts.
Reload retains the guarded TLS listener and command/authentication checks.

Reload is disabled while configuration drafts remain. A same-origin browser
checks the active revision with bounded reads before refreshing the application.
A different origin requires an explicit navigation and can require login.
A dropped response or failed confirmation never triggers an automatic activation
retry. Operator recovery remains required if listener restoration itself fails.

## Future Conversational Workflow

The user requested a chat interface for album discovery, acquisition and library
checks. Example requests include "What are the 5 most recent albums from artist x",
"Get album x from y", "get those albums" and "Do I have any of these albums".
This interface is not implemented. The model/runtime integration and its relation
to the current V1 runtime-AI exclusion need a separate scope decision before
implementation. This direction does not add chat development to the first
local album test.

For that first test only, the user accepted the closest available complete
14-track Ramones source and an original placeholder cover. Exact pressing verification
remains unknown. This does not authorize silent edition substitution in the
production acquisition workflow.

## Service Contracts Independent of the UI

The user requires services to remain useful beyond the current browser workflow.
Treat the current UI as one client, not the service specification. Reuse the
existing in-process `application::services` contracts and `ServiceHost`; do not
introduce separate servers or a generic plugin framework for this requirement.

Each remote API integration must have a dedicated client type behind an interface
abstraction. In Rust, use a client struct and an application-owned trait at the
real I/O boundary. Reuse existing provider traits where they fit; do not add a
second interface with no distinct purpose. Alternate providers and test clients
must be replaceable through host registration, without changing callers.
Keep remote requests, protocol details, response parsing and transport limits
inside the client adapter. Keep workflow policy outside it. Use the shared cache
contract and normal provider budgets rather than per-client replacements.

Keep provider operations, application policy and transport adapters separate.
Discovery, metadata, artwork and source search must not require a browser page
or an acquisition plan merely because the current screen uses one. Keep approved
acquisition, preparation and publication behind their existing plan, permission,
claim and receipt checks. Reusable reads must never become a route around those
checks. Read services may still update bounded caches and provider budgets.

Use typed requests and results with explicit filters, limits, pagination,
provider-scoped IDs and capability information. Keep unsupported operations,
unknown metadata, partial results, provider errors and retry conditions visible.
Do not assume that every provider supports every filter. Share domain rules across
HTTP, CLI and future clients instead of implementing policy in React or handlers.
Keep credentials and native paths in trusted host configuration, not client tool
arguments. No public MCP endpoint is authorized by this requirement.

Separate administrator-controlled library defaults from explicit per-operation choices.
Per-album choices must stay within the system policy when accepted-format rules
are implemented; they must not become per-user policy overrides. Freeze effective
choices when creating an approved plan. Test service contracts without React and
test adapter mapping separately. Extend contracts compatibly where possible;
never silently reinterpret saved settings, approved plans or existing receipts.

## Shared Local Cache Service

The user requires one shared local cache service for provider adapters, APIs and
other application features. Do not build separate storage, expiry and eviction
implementations for each API client. Initial uses include album covers, artist
images and reusable provider responses.

The in-process cache contract and first file-based adapter are implemented.
`application::services::cache::Cache` has provider-scoped lookup, atomic storage,
invalidation and bounded expiry eviction. `infrastructure::cache::FileCache`
uses a separately reserved, private local root and holds one OS lock for each
complete operation. API and worker adapters can share that root. Schema 21
retains all existing native bindings and permits the
new cache root kind. An explicit `cache` setting in the HTTPS or read-service host
selects this adapter for MusicBrainz responses, CAA listings, original images and normalized JPEG
variants. There is no fallback to SQLite when this selected cache fails.
Configured source reads use the same exact-release cache. There is no new cache API or
automatic root creation. Hosts without `cache` retain legacy storage.

Use the simple file-based adapter behind the small in-process cache contract.
Keep callers independent of file layout so a later storage adapter can replace
the file implementation without changing provider or UI workflows. This is a
product requirement, not a new network service. Legacy hosts retain their
existing storage until an operator selects the shared cache.

Provide shared lookup, atomic storage, expiry, invalidation and bounded eviction.
Use provider-scoped logical keys and content digests, not caller-supplied native
paths. Keep cache files in application-owned local storage outside library roots.
Bound item sizes and total storage. Coordinate concurrent readers and writers;
never expose a partial write as a valid entry. Report misses, expired entries,
corruption and storage failures distinctly. Do not silently serve stale content.

Keep provider-specific freshness, HTTP validation and provenance rules in provider
adapters. Keep provider request budgets and backoff separate from cache storage;
every network request still needs its normal permit. Share cache mechanics, not
assumptions that all providers have the same policy. Do not add arbitrary URL
fetching, cache credentials or bypass image validation through the cache.

Keep evictable entries separate from immutable approved artwork evidence, exact
JPEG previews and acquisition receipts. Eviction must never remove evidence
required by an approved plan or recovery. Preserve existing evidence bindings,
freshness checks and provider limits when moving current caches to this service.
Test cache behavior independently of React and live providers.

## Planned Tag Update, Retention and Removal Rules

The user requires future configurable rules for tags to update, tags to retain
unchanged and tags to remove. This policy is not implemented. Current
normalization writes a fresh supported tag set from the frozen exact release
and approved cover; it does not retain other source tags.

Keep these three actions explicit. Freeze the effective policy with the plan,
show its effects before approval and verify the written tags against that policy.
Retained tags must not bypass metadata validation or required output rules.
Do not change current tag behavior until the supported rules are agreed.

## Image Size Setting and Cached Variants

The user requires an image-size setting with targets of 200 x 200, 400 x 400
(recommended) or 800 x 800 pixels. The album/artist image retrieval service must
produce the scaled variant before artwork is embedded in MP3 files or written
to the album folder. Album output settings now save one of these choices and
freeze it with each new plan. Configured shared-cache hosts retain the original
CAA image and reuse content-bound JPEG variants during preview preparation.
The application-owned `ImageService` now coordinates installed artwork retrieval
with a replaceable image-processing port. The HTTPS cover-download flow uses it.
Native processing uses bounded blocking work and the same strict normalizer and
variant cache as local/cache-only preparation. Persistence rechecks the exact
original, fresh CAA evidence, frozen target, processing profile and current plan.
The service does not approve images or add a new MCP tool. Artist-image provider
installation remains open. New settings recommend 400 x 400 pixels;
legacy settings without a saved size retain the previous 1200-pixel limit.

Treat the target as a bounding box. Preserve aspect ratio without cropping,
stretching or padding. Never enlarge an image. A square original can reach the
selected square size; a non-square or smaller original retains its proportions
and may produce smaller dimensions. Show the actual output dimensions.

The retrieval service must orchestrate original retrieval, strict validation,
color conversion and downscaling through the shared image-processing path.
Keep remote API clients focused on transport and provider details. Do not
duplicate resize logic in clients, UI components, MP3 writers or publication.
The same image-processing capability must be reusable for artist images when
an artist-image provider is installed.

Use the shared cache service to retain both the original image and scaled
variants. Bind each variant to the original content digest, target dimensions,
normalization version and all output-affecting choices, including any explicit
transparency background. Keep provider identity, provenance and freshness
bindings. Reuse a valid original to create another size without downloading it
again; do not reuse stale provider evidence as fresh acquisition authority.
Apply the same size, integrity and eviction limits to cached variants.

Freeze the selected target and processing version with each new plan and bind
approval to the exact resulting JPEG. Use the same approved bytes for embedded
artwork and `folder.jpg`. Settings changes must not silently replace existing
previews, approvals or receipts. A different target needs a new preview and
approval through a new plan version. Keep retained approved evidence separate
from evictable cached originals and variants.

Acceptance must cover all three targets, square and non-square originals,
smaller originals with no enlargement, cache reuse without another fetch,
variant-key separation and unchanged existing approvals. Verify that embedded
artwork and the folder image match the exact approved JPEG bytes.

## Planned System Audio Format and Quality Policy

The administrator controls accepted audio formats and quality options for the
shared library. These are system settings, not per-user preferences.
Use a multi-select for accepted format/bitrate combinations, plus one separate
preferred format/bitrate choice from that accepted set. An ordered list alone
does not satisfy this requirement. Include MP3 CBR, MP3 VBR, FLAC and M4A in the design. M4A is less preferred
by the user; recommend keeping it disabled by default. VBR is an encoding mode,
not a fixed bitrate. M4A is a container; distinguish AAC from lossless ALAC.

The current System editor supports accepted MP3 320 kbps CBR and FLAC lossless,
with a separate preferred choice. FLAC has no fixed bitrate. New settings accept
both supported choices by default and prefer MP3 320 CBR. Old settings without
an accepted set retain both previously supported choices. New plan preparation
rejects formats outside the saved accepted set. Existing plans stay unchanged.
Additional bitrate, VBR and M4A support is not implemented. Do not enable a new option
until source assessment, complete audio validation, metadata writing and output
verification support it without transcoding.

Use one Rust implementation of accepted and preferred format/quality policy.
Freeze the policy version and selected profile with each plan. Settings changes
must not change saved approvals. Recommend presenting a lower-priority match for
review, not starting it automatically. A different selected format requires a
new plan and approval; exact edition, complete single-peer album, source rights
and exact cover approval remain separate requirements.

Search may include extension terms such as `mp3`, `flac` or `m4a` as hints.
They are not typed suffix or bitrate filters in the current protocol request.
Always filter returned extensions and metadata locally. Group results by peer
and folder and assess the complete listing before ranking eligible sources.
Missing bitrate or CBR/VBR metadata is unknown, not a match or a mismatch.
Keep partial searches and peer errors visible; a bounded search cannot prove
that the preferred quality is unavailable across the network.

## Planned Provider Throttling Review

The user requested this follow-up for the local service that contacts public
discography databases, including MusicBrainz. Review and complete graceful
request throttling before treating that service as complete. This is later work,
not part of the current local download test.

- Reuse the existing durable provider budget and backoff controls.
- Check each provider's current official request limits before implementation.
- Coordinate requests across callers. Keep pending work bounded and reuse valid
  cached results.
- Handle rate-limit responses and `Retry-After` without rapid retry loops.
  Preserve longer saved backoff and unsupported retry pauses.
- Show a clear waiting state and retry timing when known. Keep provider errors
  visible; do not show throttling as an empty search result.
- Add isolated tests for concurrent callers, throttling, backoff and recovery.
  Metadata recovery must not retry acquisition or bypass approvals.

## Brand Commitments

The name is Domus Obscura. Use shadcn/ui and a very dark, futuristic cyberpunk
direction with bright but controlled accents. The user selected **Cover wall**:
artwork-led album rows on dark panels, with electric violet for selection and
primary actions. Keep album artwork untinted. Keep amber separate for warnings.
Do not add player, streaming, rating or recommendation controls from the visual
references.

Design **Music search and exact-edition review** first. Selecting an album keeps
its exact editions and review details visible beside the results. Spectral rail
was explored but is not the selected direction. Use poster-style headings with
clear sans-serif text for album names, edition facts, forms and tables. The user selected **Wall + inspector** for the Music screen. Keep artwork
browsing and the selected album's exact-edition review visible together.

## Core Functions

Music management is one core function, not the application shell. It contains
Music and Library. The user requested exploration of a global Queue across cores,
with shared concurrency control rather than independent unbounded core queues.
The scheduler design and limits still require a separate implementation decision.
TV and Movies are future core functions with similar
user goals but separate implementations. They are not part of the current
music-only V1 implementation.

Keep each core's domain rules, providers, data contracts and workflows separate.
Share application-level facilities only where their behavior is genuinely
common. Do not force TV episodes or movies into music releases, tracks or
music-specific acquisition states. Application navigation must have a level
above the pages of an individual core. Its final form still requires approval.

The current shell studies combine tabs with a collapsible icon/text sidebar.
The application header spans the sidebar and content in every layout. Show the
application title once in that header, not again in the sidebar.
Place global Queue, notifications and the future Chat entry at the top right.
Place Account and App settings at the bottom left. Chat opens a right-side panel
in the design study only; this does not change the V1 runtime-AI exclusion.

## Confirmed Interface Design (V1)

The user confirmed this design through an interactive proof of concept. It
lives in `docs/ui-poc/` (`structure.html`, run with `node docs/ui-poc/server.mjs`).
Its **Component library** page is the visual language. The production app
must be built from shared components so one change updates every screen.
This section replaces the earlier "Wall + inspector" direction where they differ.

### Structure and navigation

- Spotify-style, search-led layout with a violet streaming-card look.
- Header (the "Orbit" header, about 60 px): Back/Forward, Home, a prominent
  search box with an animated prompt, Queue, Notifications, and the Agent icon.
  The search box is the first thing the user must notice.
- Icon-only Media sidebar. Future focus areas (TV, Movies) are shown only as
  future items. Account and App settings stay at the bottom left.
- Full-page Artist and Album views, not a split inspector.
- Search shows grouped results (artists, albums).
- Page identity: Album is violet, Artist is coral, Request is amber-tinted.
- The Agent window stays visible when open. Slide-outs dock in the remaining space.
  The Agent is a design placeholder only. Runtime AI stays outside V1.

### Home

- One standard header per application focus area (only Media now). It is
  attractive and informative and tells the user to search to start finding media.
- A suggestions flip book that cycles artists and albums. It stays empty until
  a future suggestion service exists.
- Recent searches.
- The Download queue is a shared component with four forms: collapsed, summary,
  expanded full queue, and a thin queue card. It is collapsed or summary-only
  unless something needs the user. It is important only when there is a problem.

### Album and Artist pages

- Shared hero, artist card (`.ac`) and album card (`.bc`).
- The Album page offers an edition dropdown with a star on the recommended edition.
- **Start Download** slide-out: no checkbox. The click is the approval.
  It shows live checks and a notice. An early click needs a server-side check.
  Each album is its own approval record, bound to its exact edition, cover and source.
- Never infer approval from artwork-provider edit approval or from the
  permitted-use acknowledgement.

### Queue

- Groups: "Needs your input", "In progress", "Finished".
- Status pills have an icon, one fixed width, centred text, a subtle tint,
  and extra internal padding. Verifying and Downloading are animated.
  The "Needs your input" pill has an exclamation icon everywhere.
- Finished shows 5 rows, with "Show all N" and "Clear finished". Clearing only
  tidies the list; albums stay in the library. Finished items should leave
  after 7 days. The connected app clears only the loaded queue page, including
  its finished rows behind "Show all". Other pages remain separate.
- Cancel asks for inline confirmation. A cancelled item vanishes at once.
  There is no Cancelled state. Files already saved stay.
  The real backend must never reset uncertain submissions on cancel.

### Notifications

- Problems only: "Needs attention" lists items that need input or have failed.
  The queue shows all download activity. There is no updates group.
- The bell dot shows only when something needs the user.
- Failed items offer "Open queue". Whether to add Retry is undecided.

### Request (review) page

- Hero with the amber tint. One card, "What you need to do", with Edition
  (fixed), Source and Cover rows. Each has Choose or Change. The cover list shows
  thumbnails.
- Approve, "Approve and review next (N)" and Cancel download sit in the status
  box under the rows. Approve buttons appear only when source and cover are
  chosen. Cancel download is always shown. "Approve and review next" appears
  only when other items need input.
- No output preview, no checkbox, no "View album page".

### Known UI issues (acceptable for V1)

- Narrow-screen layouts need substantial clean-up.
- The Agent-open layout is not fully checked.
- Only Chromium was checked.
- Some breadcrumbs and lists still use `/` instead of `•`.
- Radii are not fully consistent (target: 8 px buttons, 14 px cards, 999 px pills).
- A failed-notification card and the empty Notifications state were not visually checked.

## Evidence on Hand

The connected local Music browser now lives in `frontend/app`. It extends the
saved proof of concept with real guarded HTTPS requests, exact-release plans,
separate source and cover approvals, explicit one-shot acquisition and new-only
publication. See `docs/local-development.md` for setup and remaining limits.
The request page can search Cover Art Archive for the frozen exact release and
download a selected image through the installed provider service. It retains CAA
evidence and requires separate permission acknowledgement and exact JPEG approval.
Local cover upload remains available. This does not complete the approved UI port.
It is not the complete V1 application. The local setup creates a developer
certificate but never changes browser trust or enables a live account.

The connected UI now shares the approved Orbit header, Home focus header and
collapsible queue, grouped queue page, fixed-width icon status pills and fixed-size
artist/album cards. Notifications show only loaded-page problems and link to saved
decisions. Recent searches are bounded to twelve entries in the current browser
session. Album preparation opens in a native dialog styled as the download
slide-out. Safe cancellation uses one shared, explicit inline-confirmation
component and never hides an active request before server confirmation.
The request retains frozen edition/destination details in an expandable section.
Its start click is the new-only acknowledgement; the extra publication checkbox
is removed. Saved plan, source and exact-cover approvals are still independent.

The queue uses only the retained cover approved for its exact current plan.
Missing covers remain placeholders; image failures have an explicit unavailable
state. Finished-list dismissal is durable and version-bound. The server filters
finished entries after seven days from completion, not request creation.
Clearing is atomic for the selected loaded-page rows and never deletes media,
jobs, approvals or recovery receipts. Backup-pending and failed work remain visible.
These controls affect the shared queue, not a per-user queue.

Search now groups Artists and Albums with independent results, errors, manual
reloads and page cursors. Initial reads run in sequence with a one-second gap.
The server still enforces the shared provider budget and any longer backoff;
the browser never retries automatically. Artist pages load the exact provider
identity instead of using a generic heading or inferring a name from releases.
No artist-image provider is installed, so missing artist art stays explicit.

Album pages now offer an exact-edition dropdown backed by release-group browsing.
Each result must belong to the requested group. Pagination uses actual provider
rows. The current exact edition remains visible on other pages. Unofficial
editions require an explicit filter. Selection opens the exact release without
creating or approving work. Recommendation criteria remain undefined; provider
ordering does not create a recommended edition or a star.

The request now keeps the plan, source, cover and explicit start controls in one
amber review surface. Missing source and cover editors open for review. Approved
details remain available without repeating approval. Received inputs have a
separate retained-input label, not an invented source acknowledgement.
Transfer and publication receipts remain outside the approval surface.
Transfer receipts have bounded 50-row pages. Separate whole-plan saved counts
keep large albums independent of the visible page and retain planned, uncertain
submission, active and received states. Retained-input continuation requires the
complete count to match the frozen plan; it does not bypass worker checks or
authorize a transfer retry.
Stale or failed reads block commands. Saved decisions still require separate
answer and recovery actions.
Requests also expose bounded, read-only saved event history for the exact plan.
Old or unversioned events are not assigned to the current plan. History reads
cannot approve, resume or retry work.
Requests with an existing publication journal now expose a separate, explicit
new-only publication recovery confirmation. It uses retained normalized files
and the exact saved run. It never downloads, normalizes, replaces or removes
files. Current plan approval, worker fencing, staging lock and destination
identity checks remain authoritative. An offline backup is not completion.
This does not implement replacement, restore or cleanup.
The foreground API stops admitting local operations on shutdown and drains the
active operation through result recording instead of abandoning its blocking
task. Ctrl+C is supported; Linux SIGTERM is wired to the same path. Forced
termination and native service-manager acceptance remain unverified.

The read-only Library screen now offers an explicit saved-index refresh,
root-bound folder navigation and clear incomplete-coverage and failed-refresh
states. It does not scan or change media. Changing a resource key immediately
hides the old resource until the new read starts. This also protects request
and provider views from showing another resource's saved result as current.
The Library is still a folder index, not a completed tag-based album browser.
An explicit root-availability check opens only the configured root through the
existing no-follow boundary and verifies its saved native binding. It does not
scan, update coverage or authorize later writes. Missing, inaccessible, unbound
or replaced roots remain errors; cached browsing remains available.

This remains a partial port: recommendation policy and other retained V1 work
remain open. The original HTML study is unchanged and remains authoritative.

Generated regression tests cover anonymous HTTPS approval through local MP3
normalization/publication, exact folders/filenames, all required tags and unchanged
encoded audio. Separate generated protocol-socket tests cover MP3/FLAC receive
through publication. This is not new live-account acceptance.

Issue #1 records the approved product scope. The current implementation is a local foundation, not the complete web application. One user-approved live 14-track MP3 test passed acquisition, normalization and private playback export on Windows. Independent checks verified names, tags, artwork and output receipts. The user confirmed clear first-track playback and the test cover in the desktop player. Exact pressing verification and production deployment remain unverified.

## Product Principles

- Keep each decision explainable and tied to unchanged inputs.
- Prefer verified files over success-shaped progress messages.
- Preserve existing content before replacement.
- Keep resource use bounded.
- Use simple code and one implementation of each policy.

## Accessibility & Inclusion

Support keyboard use, visible focus, clear text status, narrow screens, and reduced motion. Do not use color or glow as the only status cue.
