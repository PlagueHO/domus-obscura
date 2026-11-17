# Application persistence structure

## Status

**Storage policy agreed; implementation remains pending.** The user
selected one private application directory with overridable child folders and
separate media libraries. Primary and backup libraries can be local folders,
network file shares, or other native filesystem locations. They are not limited
to the application directory, its disk, or local storage.

The seven core decisions, SQLite temporary folder and state-bootstrap location below are
confirmed. Detailed configuration fields and
filesystem/mount acceptance still need implementation and verification.
Application code, existing folders, and binding rules remain unchanged.
This agreement does not authorize data deletion or changes to existing media.

This document describes where data belongs. The separate
[storage-client experiment](storage.md) describes filesystem identity and mount
safety. A folder layout does not resolve those safety questions.

## Agreed default model

Use one private application storage directory with clearly separated child
folders. Use these folders as defaults, not mandatory locations. Keep explicit
path overrides where separate disks or existing installations need them.
Configure primary and backup media libraries separately. Either role can use
local or network storage. Do not move existing music.

`<application-storage>` means the operator-selected private application
directory. It is not a library folder.

```text
<application-storage>
  config.json                            host/startup configuration only
  secrets
    <provider credential files>
    <identity client secret, when configured>
    <manual TLS material, when configured>
  state
    state.db
    state.db-wal                         when SQLite requires it
    state.db-shm                         when SQLite requires it
    sqlite-tmp
      <SQLite-owned temporary and query-spill files>
    <stable process lock files>
  cache
    owner.lock
    <provider-scoped, content-bound entries>
    <pending cache replacement, if present>
  staging
    owner.lock
    transfer-<id>.input
    transfer-<id>.receipt
    normalization-<id>
      recipe.json
      <normalized audio files>
      folder.jpg
    playback-<id>                        existing playback-test export
      <exported album files>
      complete.json
  certificates
    lifecycle.json                       optional managed TLS state
    generation-<id>.json
    <stable lifecycle lock>
    <pending, archived, and recovery evidence>

<primary-library>                        separate local folder or network share
  <published album folders, audio, artwork, and existing library files>
  <publication working folders, when required>
  <retained or restore-displaced albums, when required>

<backup-library-N>                       independently configured destination
  <published album folders, audio, artwork, and existing library files>
  <publication working folders, when required>
  <retained or restore-displaced albums, when required>

<application-state-backup-destination>   optional operator-managed backup
  <consistent private-state backup sets>

<application-installation>
  <application executables>
  browser
    <packaged browser files>

<browser-managed storage>
  <authentication cookie, in secure mode>

<host-managed logs>
  <captured console or service output, if the operator retains it>
```

This is a logical map, not a requirement to create every optional file.
Existing implementation names are shown for staging and certificate records.
This issue must not silently move or reset existing state.
Explicit secret-file and manual TLS references can remain outside the private
application directory.

## Main configuration ownership

**Confirmed name and location:** `<application-storage>/config.json`.
This names the file by its purpose. `service.json` suggests one service, but
the API, worker, and CLI share the application configuration. A separate
`config` folder adds little value for one main file.

**Confirmed requirement:** This file contains only host/startup configuration:
storage paths, listeners, authentication, provider setup, and secret-file
references. Application components must treat an existing file as
operator-managed and read-only. The API, workers, CLI commands, and
browser commands must not change its settings. Keep settings that the
application can change, such as shared output policy, in the existing versioned
SQLite store. Audio policy, artwork size, and output templates remain
system-wide administrator-controlled policy, not per-user settings.
Do not copy those mutable settings, jobs, plans, approvals, or runtime state
into this file.

**Confirmed state-bootstrap exception:** The operator-approved expected native
state-root binding belongs in `config.json`. It is startup trust information,
not mutable application policy or a copy of SQLite records.
Startup must verify this expectation before SQLite can create coordination files,
recover or migrate. The provider scope and persistent identity format still need
proof. A copied database or owner marker must not authorize a replacement root.
Changing the binding requires explicit operator action and coordinated restart.

Operator edits alone do not synchronize running processes. Each process can
retain a different active snapshot after an edit or a partial restart.
**Confirmed update procedure:** Stop the API and workers at safe
boundaries, edit and validate the operator configuration, then start all
components with that same configuration. Do not use a file watcher or an
API-only reload as proof that every component has applied a change. Remove
independent host-configuration reload. Normal versioned SQLite commands and
managed certificate renewal remain separate from host-configuration changes.

**Current implementation differs:** It supports guarded JSON saves and an
explicit API-host reload. JSON saves conflict with the new no-application-writes
requirement. An API-only reload leaves independent workers unchanged.
These differences require later implementation work. Existing
configuration paths, lock files, and pending-write files are not changed by
this document. The target layout does not introduce normal application-managed
configuration update files.

## Confirmed decisions

| Decision                       | Selected policy                                                                                                                                                                                                                       |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Main configuration             | `<application-storage>/config.json`, at the root.                                                                                                                                                                                     |
| Default application storage    | OS data directories for personal use and installed services; container destinations follow the container OS.                                                                                                                          |
| Path resolution                | An explicit startup storage-root option overrides the OS default. Resolve relative children from that root, never the working directory. Use absolute overrides outside it.                                                           |
| Configuration activation       | Stop the API and workers safely, edit and validate host configuration, then restart all components with the same configuration. No independent host-config reload.                                                                    |
| First launch                   | Create a safe starter configuration once if none exists. Use loopback-only single-user access with external providers disabled. Never replace an existing file; invalid existing configuration fails explicitly.                      |
| Incompatible development state | Breaking changes are allowed. Report incompatibility and offer explicit fresh setup. Migration is not required for disposable pre-production state. Never reset or delete automatically.                                              |
| Initial Docker support         | Start with documented, verified layouts: one persistent application-storage mount shared by API and worker, plus separate media mounts. Admit private child-folder overrides only where their arrangement passes the required checks. |
| SQLite temporary files         | Use a fixed private `sqlite-tmp` child beneath the selected state directory. It follows a state-directory override. Do not add an independent setting or use the OS temporary directory as a fallback.                                |
| State bootstrap binding        | Keep the operator-approved expected native state-root binding in `config.json`, outside SQLite. Verify it before any creating, coordinating, recovering or migrating database open. Its durable provider scope remains a proof gate.  |

These decisions close the earlier product questions. They do not fix the
existing-code findings in the separate implementation compatibility review.

The temporary-folder decision adds a location, not a new cleanup command.
SQLite manages its own temporary files. Do not delete unknown files or uncertain
outputs during startup. Temporary files are not a backup of durable state.
The isolated TEMP/sort proof is recorded in [storage.md](storage.md).

## What the shared SQLite database contains

These are record groups inside `state.db`, not extra directories or databases:

| Record group                     | Persistent information                                                                                                                      |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Application and storage identity | Application identity, registered primary/backup locations, native root bindings, and private-root reservations.                             |
| Library index                    | Files, folders, artists, albums, extracted metadata, observed identities, coverage, scan progress, and scan errors.                         |
| Shared system policy             | Versioned output settings, audio policy, artwork settings, and output templates. Backend host configuration remains in JSON.                |
| Acquisition and work             | Album requests, queue state, jobs, worker claims and leases, transfer intent, source candidates, and selected exact-release evidence.       |
| Plans and approvals              | Immutable plans and settings snapshots, approval records, approved JPEG bytes, and related hashes.                                          |
| Operation and recovery journals  | Transfer, normalization, publication, backup, replacement, restore, cleanup, and deletion progress and receipts.                            |
| Decisions and events             | Inbox questions, answers, saved decisions, and durable operation/session events.                                                            |
| Provider control                 | Request scheduling, cooldowns, pauses, and related durable evidence. Provider passwords do not belong here.                                 |
| Sharing and artist images        | Explicit folder bindings, sharing visibility and generated catalogs, and artist-photo bindings.                                             |
| Browser sessions                 | Session-token hashes, authorized identity references, expiry, and session events. Not raw bearer credentials.                               |
| Legacy cache compatibility       | Existing SQLite provider-cache records where a legacy workflow selects them. No automatic migration or fallback from a selected file cache. |

Some index and catalog records can be rebuilt through supported operations.
The database as a whole is not disposable. Jobs, approvals, receipts, and
recovery history must survive application and container replacement.
All application processes share this one database. Do not add a separate
database per process or data category.

The [SQLite migrations](../crates/domus/migrations),
[staging adapter](../crates/domus/src/infrastructure/staging.rs),
[playback export](../crates/domus/src/infrastructure/staging/playback.rs), and
[certificate state adapter](../crates/domus/src/infrastructure/certificates/state.rs)
define the existing record and file layouts.

## Folder purposes

| Location                 | Contents                                                                                                                                                                                        | Retention                                                                                                                                |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `config.json`            | Operator-managed backend host JSON. Keep versioned library output policy in SQLite, not in a second JSON copy.                                                                                  | Keep and back up. Under the agreed requirement, application components must not write it.                                                |
| `secrets`                | Operator-supplied provider passwords, client secrets, or other protected credential files. Existing external secret-file references can remain external.                                        | Keep private and back up securely. Do not return contents to the browser.                                                                |
| `state`                  | The shared local SQLite database, its coordination files, and stable process locks. SQLite retains jobs, settings, plans, approvals, approved artwork, receipts, and recovery records.          | Essential persistent state. Use the existing consistent backup procedure. Do not delete lock files as cleanup.                           |
| `cache`                  | Bounded provider responses and derived images that the cache policy can remove. No sole copy of approvals, approved JPEG evidence, plans, or receipts.                                          | Cache entries can be rebuilt. Do not remove the root or its ownership/lock files while processes use it.                                 |
| `staging`                | Received audio, normalization output, and input retained for retries or unfinished backups.                                                                                                     | Not disposable cache. Keep required files through interruption and recovery. Remove only under the existing receipt-bound cleanup rules. |
| `certificates`           | Optional managed TLS account state, issued generations, keys, and recovery records. It is unused when managed TLS is not configured.                                                            | Keep private and back up securely when used. It is separate from the provider cache.                                                     |
| Media libraries          | Existing main and backup music folders on local storage, network file shares, or other native filesystem locations. Registration permits read-only browsing and scanning, not automatic writes. | Keep independently of application installation and cache. Publication still needs separate approval.                                     |
| Application installation | Executables and packaged browser files.                                                                                                                                                         | Replaceable program files. Do not mix them with mutable state or secrets.                                                                |

This proposal does not add file logging, automatic backups, or a new export
feature. Existing playback-test exports are stored under
`staging/playback-<id>` and remain protected from normal scratch cleanup.
They are not a new independent export root.

## Persistence outside the private application directory

**Media libraries** hold the actual music and artwork. Primary and backup
destinations can be local or network locations. Their paths are
deployment-specific settings, not hardcoded software defaults. A backup media
library is not a backup of SQLite, configuration, or secrets. This layout does
not add a general filesystem mirroring service.

**Application-state backups** are operator-managed consistent backup sets at a
separately selected destination. Follow the existing backup procedure for
SQLite, configuration, required secrets, managed certificate state, and
unfinished-work files. The layout does not create an automatic backup schedule
or permit copying only a live `state.db` while ignoring its coordination files.

**Program files** are installed separately. Executables and browser assets are
replaceable application files, not user state. Updating them must not replace
private state or libraries.

**Browser state** can include the authentication cookie in secure mode.
The server stores the corresponding session hash, not the raw session
credential. Anonymous HTTP mode does not acquire secure-mode authentication
cookies. The current connected frontend does not use `localStorage`,
`sessionStorage`, or IndexedDB as a persistence store for application data.

**Logs** remain console/service output. The host can retain that output under
its own logging policy. There is no new application `logs` directory in this
proposal. Secrets and bearer credentials must not enter logs.

## Information that is not persisted

Open directory handles, active OS lock ownership, network connections, decoder
buffers, pending login PKCE secrets and nonces, and unsaved browser drafts are
process/browser memory, not new files or SQLite records. Job intent, progress,
and recovery evidence still persist in SQLite. Stable lock files can remain on
disk, but their presence does not mean a process still owns the OS lock.
Saved configuration and each process's active configuration remain distinct.

## Recovery data is not one disposable folder

SQLite keeps the recovery journal and receipts in `state`.
Required transfer and normalization files remain in `staging`.
Retained or displaced albums remain at their affected library destination under
the existing publication/recovery naming rules.

Do not move retained albums into `cache` or a central application recovery
directory. Do not treat all staging files as temporary files that can be cleared
at restart. Restore and irreversible deletion keep their separate approvals.

## Shared use and configuration

The API, worker, and CLI must use the same selected storage and shared database.
Do not give each process an independent copy of `state`, `cache`, or staging
data. Existing staging ownership rules still permit only its legitimate owner.
SQLite stays on local disk.

The agreed default model is one parent directory with child defaults. Explicit
overrides would allow an operator to place a large staging directory or cache
on another supported disk. Media roots always remain separate configuration
entries. Exact JSON field names are not selected yet.

The local-disk requirement for SQLite does not apply to media libraries.
Windows UNC shares and network shares mounted on Linux are examples of media
locations. A network library does not require a separate local copy.
Unavailable network storage must remain unavailable or incomplete, not be
reported as confirmed absence.

The storage design must support network-library locations without weakening
native identity, no-follow traversal, locking, publication, or recovery rules.
Actual support for a filesystem or share requires separate verification.
If it cannot supply the required guarantees, report the unsupported condition.
This agreed requirement is not a claim that every network filesystem has
already passed those checks.

All private folders must remain separate from media. State, cache, staging, and
managed certificate state must also keep their existing mutual exclusions.
Different folder names do not prove that mounts refer to separate storage.
Reject a configured relationship when that separation cannot be established.

## Native and container defaults

| Deployment                                  | Default application storage                                                        |
| ------------------------------------------- | ---------------------------------------------------------------------------------- |
| Windows personal use                        | `%LOCALAPPDATA%\DomusObscura`                                                      |
| Windows installed services                  | `%PROGRAMDATA%\DomusObscura`                                                       |
| Linux personal use                          | `$XDG_DATA_HOME/domus-obscura`, falling back to `$HOME/.local/share/domus-obscura` |
| Linux installed services                    | `/var/lib/domus-obscura`                                                           |
| Linux containers, on Linux or Windows hosts | `/var/lib/domus-obscura`                                                           |
| Windows containers                          | `C:\ProgramData\DomusObscura`                                                      |

These are defaults, not mandatory locations. Explicit startup overrides remain
available. Choosing service defaults does not install services, create accounts,
or grant host permissions.

Containers must retain the required application data outside the replaceable
container filesystem. They can present the same logical structure through
mounted storage. Docker startup configuration, such as Compose or `docker run`,
declares the mounts; the application does not create host mounts. The host
determines source paths, while the container OS determines destination paths.
A Linux container on a Windows host still uses the Linux destination.
Use a Docker-managed local volume or an explicit local host directory for
application state. Reuse it when replacing containers. Choosing a destination
path alone does not make data persistent. Media mounts remain separate and can
be read-only for browsing.

The layout is an agreed target, not proof that either container image is ready.
The boundary experiment found that stable identities alone do not prove
media/private separation across aliases. Mount validation remains a separate
design gate. Verify Linux and Windows container images, permissions, identities,
locking, and recreation separately. Reject unsupported arrangements explicitly.
See [Windows container storage](https://learn.microsoft.com/en-us/virtualization/windowscontainers/manage-containers/persistent-storage)
for platform-specific mount behavior.

## Implementation details and evidence still open

The seven policy decisions are complete. Detailed Rust/JSON contracts, private
root protection, local-filesystem checks, and the verified deployment matrix
remain engineering work. The boundary-only design failed. A replacement using
handle-correlated Linux mount evidence now passes a restricted same-ext4
experiment, including the real hidden alias and a child bind-mount crossing.
See the [replacement proof and limits](storage.md#replacement-proof).
The native Windows experiment also passes a restricted same-volume NTFS check
using retained handles and normalized volume GUID coordinates, with actual
alias, junction and replacement fixtures. These experiments do not prove
network-share, different-filesystem, native Linux-host, or Windows-container
support. A separate Ubuntu 24.04 VM now passes the bounded SQLite and Rust
ownership proof. That does not prove native Linux storage identity or backing
support. The approved disposable SMB lab reproduced different volume serials
for two exports of the same backing directory. After separate approval to load
CIFS in Docker, actual retained client handles also reproduced changed native
device identities across containers and recreation. Unknown remote layouts are
rejected. Positive network admission still needs a stable identity and relationship
witness. The [network candidate review](storage.md#network-identity-candidate-review)
also rejects Samba's share-name object IDs and the reviewed Linux path-reopening
query ioctl as generic witnesses. SMB normalized names do not supply common
cross-export ancestry. These are source findings, not additional mounted
acceptance. Runtime integration still needs separate review.

The user has approved investigation of a
[server-side metadata witness](storage.md#server-side-witness-design-candidate).
The candidate requires evidence tied to the same retained remote and native open.
It does not yet prove cross-provider separation or subsequent file-operation
safety. Approval covers design, not installation, new services or runtime changes.
The subsequent approved disposable-lab experiment now
[matches retained SMB opens to full native identities](storage.md#measured-retained-smb-open-bridge),
rejects replaced roots and invalid opens, and reads native filesystem UUIDs
across server recreation. It does not yet provide an authenticated production
witness or complete network admission.
The separately approved
[read-only native server hook](storage.md#measured-read-only-native-server-hook)
now proves restricted parent-relative access and refusal during controlled
parent/child replacement and an actual child mount crossing. It uses generated
Linux/ext4 fixtures and a matching source-built Samba server only.
The subsequent approved
[authenticated witness experiment](storage.md#measured-authenticated-native-witness)
verifies encrypted SMB and operator-pinned signatures over actual native
identity, UUID, ancestry and relationships. It also verifies hidden aliases,
changed-root refusal, expiry, restart, key rotation and missing-key refusal.
This remains a fixed Linux/ext4 diagnostic. General NAS compatibility,
cross-provider and client-local/remote separation, durable binding and production
file operations remain open. No application binding behavior has changed.

Backward compatibility with old configuration, CLI forms, or disposable test
state is not required. Explicit fresh setup must leave old data and existing
media untouched. Any optional transition that retains valuable bound state
still needs exact-state safety and separate approval.
