# Product

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

## Proposed MP3 VBR Support

The user requested consideration of MP3 variable bitrate (VBR) as a separate
requested quality option because many source results advertise VBR. This is not
yet approved or implemented. Keep the current MP3 320 kbps CBR validation unchanged.
Define the VBR quality criteria and update source assessment, frozen plans,
audio validation and shared contracts together before accepting VBR.

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
  after 7 days (not built).
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
