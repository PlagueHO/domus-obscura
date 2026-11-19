# Docker distribution

## Scope

The image contains the current Rust API and worker, the real connected React
browser, migrations, dependency notices, and package provenance. Node.js and Rust
are build tools, not application runtime requirements.
This packages the current foundation. It does not complete V1 or approve public
API access.

The planned target is one `linux/amd64` image on Linux Docker Engine or Windows Docker Desktop in
Linux-container mode. Native Windows containers, ARM64, and network-hosted SQLite
are not supported. The runtime uses Ubuntu 24.04 and numeric user/group `1000:1000`.

The API requires direct loopback access. Use host networking, not `-p`, a reverse
proxy, or an anonymous public listener. All Host, Origin, CSRF, TLS, connection,
and native filesystem guards remain active.

Docker Desktop requires version 4.34 or later with **Enable host networking**
selected in **Settings > Resources > Network**. Enhanced Container Isolation
must be disabled for this Docker feature. Review that isolation trade-off before
changing a host. These settings require separate operator approval.
See [Docker host networking](https://docs.docker.com/engine/network/drivers/host/)
and the [Windows WSL backend](https://docs.docker.com/desktop/features/wsl/).

## Acceptance status

The Docker branch is rebased onto main at
`4f8eafcabc668dc5c4869c0b499a31b87694e975`, which includes merged PR #70
and its final native setup fixes. The earlier exact-dependency integration
remains historical evidence, not proof for the current rebased source.
The current image and Docker workflows need their own checks.
Native storage checks are not whole-image Docker acceptance.

The previously built `f497e7e` image passes generated-volume browser-byte and API-guard
checks, worker scanning, selected bindings, overlap refusal, recreation, graceful
stop, forced exit, invalid configuration, and replacement refusal.
The image build also passes the full native archive lifecycle check on a fresh
local volume. These are engine results, not supported-host acceptance.
After the separately approved Docker Desktop host-network setup and engine
restart, Windows host access to the existing candidate passes the real API
session, packaged browser-byte, command-guard and state-recreation checks.
Report: `target/docker-windows-loopback.json`. Its scope is API loopback only,
using image source `f497e7ea4c7ecd0dfd260a332eb39d96b7e3ad67`. It does not
establish external-media or whole-image Windows acceptance.
The storage dependency replaces the old container-ancestor binding with the
approved operator-selected filesystem contract. Do not restore the old binding
model or remove the new admission checks. See the [storage contract](storage.md).

The generated Windows-folder capability probe found a 9p mount. Component-relative
no-follow opens rejected a generated junction and symlink. Live alias identities
matched, and replacing a stopped parent changed its observed identity without
changing the retained leaf. However, the mount refused `name_to_handle_at` with
errno 95 and `FS_IOC_GETVERSION` with errno 25. No supported persistent native
identity profile was established. Runtime inode observations are not a saved
authority fallback. External folder mounts remain required; Linux-private volume
checks do not replace this gate.

On this host, Docker CLI and Compose created missing Windows bind-source
directories. This occurred despite `create_host_path: false` and an additional
CLI check with `bind-create-src=false`. This remains a separate runtime gate.
Native Linux host acceptance and a public registry pull are still unverified.
Do not use this image with an existing library until these gates pass.

## Native media adapter proof

The user approved implementation of a native Windows media adapter for the
Linux image. External Windows folders remain in scope. This changes their
planned access from guest 9p mounts to authenticated native provider roots.
It does not approve host changes, installation, or service registration.

The read-only implementation is `infrastructure::filesystem::media_adapter`.
The explicit `media-adapter` foreground command can now start its listener from
operator-managed configuration. Host-selected paired roots can now be registered
explicitly and checked through the application's location availability operation.
Full library scans and direct-folder refreshes use the retained read-only client.
JPEG reads, sharing, publication and recovery are not routed through the adapter.
Do not treat this implementation as Windows Docker acceptance.

The adapter verifies complete selected native chains and retains the opened
directories and files. Only the Windows/native host maps root IDs to paths.
The client sends frozen root definitions and a logical selection digest.
References belong to one authenticated connection. Closing a parent closes its
descendants. A new connection or adapter restart cannot reuse those references.
Private adapter directories and supplied private-input hard-link aliases are
excluded. Configuration and TLS inputs remain in memory after preparation.
The adapter cannot open a remote database or mutate media.

The loopback-only channel requires mutual TLS and exact paired leaf certificates
on both sides. Pinning supplements normal chain, validity, usage and hostname
checks. It does not replace them. The listener reuses the guarded API server.
It rejects browser headers and bounds request time, frames, pages, reads,
blocking tasks and live references. A failed enumeration remains incomplete.
Connection loss never becomes an empty page or a fallback to 9p.
Each connection permits 32 live references. Pages have at most 64 entries.
File chunks are at most 16 KiB, and frames are at most 256 KiB.
At most eight native operations run at once. Each request has a 10-second
deadline. A deadline closes the connection; it cannot cancel an OS read already
running on a bounded blocking task.

Run the generated native and TLS fixtures on Windows:

```powershell
.\dev.ps1 test media_adapter
```

For a separate container-to-host channel diagnostic, use the existing
`domus-obscura:storage-candidate` image:

```powershell
.\dev.ps1 -TaskArgs @('test', '--lib', 'media_adapter_docker_loopback_probe', '--', '--ignored')
```

The diagnostic does not pull an image or change Docker settings. It starts a
generated Windows fixture and one temporary container. Only generated pairing
material enters the container's temporary filesystem. Its report is
`target/media-adapter-docker-loopback.json`. A successful result proves only the
authenticated root-open channel, not production provider integration or complete
Windows Docker acceptance. The ordinary test suite skips this diagnostic.

The configured adapter adds generated checks for passive preparation, frozen
activation, private-input alias refusal and a latched stop before startup.
The first
Linux-container diagnostic failed with curl exit 7: connection refused.
The user then explicitly approved enabling Docker Desktop host networking and
restarting its engine. The saved setting is now on and the engine is running.
Enhanced Container Isolation remains off and was not changed.
The repeated diagnostic passes native Windows mutual TLS and an authenticated
root-open request from the Linux container. Its generated CA has a distinct
issuer name so both WebPKI and the image's curl can validate the test chain.
Certificate verification and exact peer pinning remain enabled.
Generated containers, fixtures and the API check's private volume were removed.

These fixtures need no service installation, live provider, or existing library.
The remaining production media consumers, approved mutation/recovery operations,
and whole-image supported-host checks remain required. The successful channel
diagnostic does not place the adapter in the cached Linux image or route
production operations through it. SQLite and other private application storage
retain their existing local-only rules.

### Configure the native foreground adapter

The adapter uses a separate operator-managed JSON document. It does not open
SQLite or use the Linux application's `config.json` as a Windows host file.
No command creates, adopts, rewrites or rebinds an adapter selection.
Review the native roots with `storage-root-inspect` before writing the document.
See the [storage contract](storage.md) for complete native expectations.

The document has these required fields. Unknown or duplicate fields fail.

| Field                | Meaning                                                                            |
| -------------------- | ---------------------------------------------------------------------------------- |
| `format_version`     | `1`.                                                                               |
| `listen`             | A loopback socket address with an explicit nonzero port, such as `127.0.0.1:8088`. |
| `selection`          | The frozen adapter selection described below.                                      |
| `certificate_file`   | The server certificate chain, including its issuing CA.                            |
| `private_key_file`   | The matching server private key.                                                   |
| `client_ca_file`     | Trust anchors for client certificate verification.                                 |
| `paired_client_file` | The exact paired client certificate and any required intermediate certificates.    |

`selection` contains a positive `revision`, one `provider` mapping, a nonempty
`media` root array and one `private` root. The provider has a scope and an
absolute native boundary. Each root uses its full inspected `StorageRoot`
record. Media roots must have role `media`; the private root must have role
`application`. The scope, roles, revisions and complete expected chains must
match. Known overlaps and changed identities fail.

The document must be directly inside its selected private root. All four TLS
file references are nonempty `LibraryPath` identifiers beneath that root.
Use slash-separated relative identifiers, not absolute Windows paths.
Links, reparse traversal, alternate streams and root escapes fail. The opened
private directories and files must have owner-only access on supported local
storage. The document is limited to 256 KiB. Certificates and CA bundles are
limited to 64 KiB each. The private key retains the existing 16 KiB TLS limit.
The adapter retains all five input handles and rejects their hard-link aliases
through media exports. Private keys never enter the database or protocol.

Validate without opening a listener, then run explicitly in the foreground:

```powershell
domus media-adapter-check D:\DomusAdapter\private\adapter.json
domus media-adapter D:\DomusAdapter\private\adapter.json
```

These are operator examples, not permission to change an existing host.
The commands do not install or register a service. Stop with Ctrl+C.
On Unix, SIGTERM also requests a stop. Shutdown stops new admission and gives
active read-only requests up to ten seconds to drain. Stop before editing the
document or TLS files. Restart to activate changes; there is no live reload,
automatic pairing, certificate generation or fallback provider.

### Configure the paired read-only client

`MediaClient` provides the same bounded read-only operations for native and
paired TLS sessions. Directory and file leases are opaque and belong to one
client. Closing a directory invalidates its descendants. Cancellation, transport
loss or inconsistent peer evidence retires the session. Recreate it explicitly;
never resume a lost cursor or reopen a Windows file through a guest mount.
Foreign identity records are checked as peer declarations only. They are never
admitted to local filesystem access.

The client uses a separate operator-managed document beneath the main host's
admitted private inputs. Load it through `ConfigurationService` with the main
host configuration. The client document alone does not register a media location.
Unknown or duplicate fields fail.

| Field                | Meaning                                                                         |
| -------------------- | ------------------------------------------------------------------------------- |
| `format_version`     | `1`.                                                                            |
| `address`            | The adapter's fixed loopback socket address with a nonzero port.                |
| `selection`          | The exact logical selection described below.                                    |
| `server_ca_file`     | Trust anchors for server certificate verification.                              |
| `paired_server_file` | The exact paired server certificate and any required intermediate certificates. |
| `certificate_file`   | The client certificate chain.                                                   |
| `private_key_file`   | The matching client private key.                                                |

The logical selection has `revision`, `provider_scope`, `media` and `private`.
These fields must match the native adapter's frozen declaration. The native
`provider.boundary` path is not a client field. Complete native expectations
remain in each root record. The peer must verify them before issuing a lease.
Known duplicate or nested roots fail before a connection opens.

All TLS references are nonempty `LibraryPath` identifiers beneath the client
document's directory. Private local-only admission and file permissions remain
required. The limits remain 256 KiB for the document, 64 KiB for certificates
and CA bundles, and 16 KiB for the private key.

```powershell
domus media-client-check --host D:\DomusData\config.json D:\DomusData\media-client.json
domus media-root-check --host D:\DomusData\config.json D:\DomusData\media-client.json library
```

The first command validates without contacting the peer. The second explicitly
opens and closes one selected root over authenticated TLS. It fails if the
adapter is unavailable, the selection differs or the root ID is not selected.
Neither command scans, registers a location, starts a worker or mutates media.
These checks do not establish whole-image or external-folder lifecycle acceptance.

### Select and register a paired library root

The main host document can select one paired provider with `paired_media`.
Its required fields are `client_file` and `topology_assurances`. The client file
is a private local path, resolved relative to the main host document. Its
contents and pairing material are frozen during host preparation. Each later
availability operation opens its own authenticated session. It does not reread
the client document or share a live connection with another operation.

Keep all local private roots in the existing `storage` selection.
Do not put foreign media expectations into that local selection or give them a
dummy native path. The paired provider scope and all root IDs must be distinct
from the local provider scopes and root IDs.

Before adding `paired_media`, obtain the exact review digest from the stopped
host's existing local selection and the candidate client document:

```powershell
domus media-storage-digest --host D:\DomusData\config.json D:\DomusData\media-client.json
```

This command reads private configuration. It does not contact the peer or
approve a topology. For each paired media root and each local root, the operator
must supply a `TopologyAssurance` with `left`, `right` and `selection_digest`.
`left` and `right` each contain `root_id` and `binding_revision`. Order the two
root IDs lexically. Use the printed digest for `selection_digest`. Also supply
an assurance between the adapter's private root and each local media root.
Do not add private-to-private assurances.

These are explicit operator declarations that the selected namespaces do not
overlap. Review the real host/share mappings before making them. Every required
pair must appear exactly once. Unknown roots, stale revisions, wrong digests,
duplicate pairs and known native overlap fail. An assurance cannot override a
known overlap. This uses logical filesystem mappings, not physical-device
identity. The main host document retains its existing 16 KiB limit.

Add the reviewed `paired_media` section while all state users are stopped.
Validate the saved host configuration, then restart all components together.
Passive host preparation does not contact the adapter. Missing or invalid
selected client configuration fails; there is no provider fallback.

Register one selected root explicitly, then check its saved binding:

```powershell
domus location-add-paired --host D:\DomusData\config.json library main
domus location-check --host D:\DomusData\config.json 1
```

Registration requires writable system settings and an authenticated root-open
and close before its database transaction. It saves only the provider/root
definition, not Windows paths, TLS keys or live tokens. Existing limits of
100 locations and one main location remain. A saved root cannot be replaced.
An unrelated selection revision can change without rewriting that root.
Changing its provider, binding revision or complete expectation fails against
the saved record. Removing a provider with registered roots also fails.

The existing API location-availability operation uses the same saved binding.
It performs network I/O on a blocking thread without a SQL transaction.
Peer loss does not change indexed entries, coverage or observation dates.
Native filesystem and publication accessors reject paired roots explicitly.
Full scans and direct-folder refreshes enumerate this provider in bounded pages.
Each descent opens the exact listed object relative to its retained parent.
MP3 and FLAC inspection uses the same bounded tag parser as native scans through
retained-file Read/Seek. Reads are limited to 16 KiB per adapter request.
The existing 8 MiB metadata envelope, frame/comment limits and 64 KiB MPEG probe
remain. This inspects browsing metadata, not full audio validity.

The existing persistent scan frontier, ownership checks and direct-folder scope
remain active. Each page, open and read checks worker stop and renews the claim.
Short persistence transactions contain at most 128 entries. They never span
network I/O. Stop releases the claim for explicit continuation. Connection loss,
changed objects, unsafe traversal and failed enumeration leave incomplete
coverage. They do not mark unseen entries absent. An unsafe directory entry
currently fails its paired enumeration rather than producing a native-style
`unsafe_link` row. Invalid tags and unsupported audio remain indexed file
diagnostics. Recovery folders stay excluded.

JPEG reads, sharing, publication and recovery are not yet routed through this
provider. Their native-only accessors still fail explicitly. These limits remain
required work for issue #66.

## Build and verify

From a checkout on Windows:

```powershell
.\dev.ps1 docker-build
.\dev.ps1 docker-check
```

On Linux:

```sh
cargo task docker-build
cargo task docker-check
```

Docker, Python 3, and the repository task launcher must already be available.
The Docker build installs its build tools inside the build image only.
It does not install host software or change Docker Desktop settings.

The local tag is `domus-obscura:local`. The image bases and language tools are
pinned. Dependency restores use the checked-in lockfiles and the default package
registry. The task runs the existing Linux archive and process checks.
BuildKit caches can use unsupported overlayfs storage. Archive structure checks
run during image assembly. The unchanged lifecycle checker then runs in a
temporary verification image, with fresh state on a Docker local volume.
The task exports the runtime image only after those checks pass. The temporary
container, image and volume are removed. A raw `docker build` does not replace
this verification task.
The native SQLite wrapper downloads and verifies its pinned upstream archive.
Complete SQLite or SQLCipher sources are not committed or copied into the runtime.
The source context excludes Git worktree metadata, private state, credentials,
and generated build outputs.

The native package manifest records a supplied commit and `dirty: null` for an
image build without Git metadata. This means the build cannot inspect the source
checkout. The image's `io.domus.source.dirty` label records the launcher's source
status. Neither field authenticates a publisher. Release workflows require a
clean checkout and both host results for the same image.

`docker-check` uses fresh volumes and a small generated media directory.
It checks the real browser/API, command guards, a read-only mount, a worker scan,
native bindings, state after recreation, worker contention, safe stop, forced
exit, invalid configuration, and rejected replacement roots.
On Windows, it also checks a generated junction to a directory outside the
mounted library. It never uses an existing library or live credentials.

Results are written to `target/docker-check.json`. The runner deletes only its
named test containers, volumes, and generated directory. It does not prune Docker.
A separate diagnostic can check the Linux engine without claiming host access:

```powershell
python packaging\docker\check.py check --engine-only --report target\docker-engine-check.json
```

An engine-only result is not Windows-host acceptance and cannot authorize release.
Image creation, native package checks, and Compose parsing are also not runtime
acceptance.

An additional diagnostic isolates Linux-private media from Windows filesystem
translation:

```powershell
python packaging\docker\check.py check --engine-only --volume-media --report target\docker-linux-volume-engine.json
```

This mode also cannot authorize release or prove Windows bind-mount safety.

## Run the API

Use `compose.yaml` with a trusted image. For a published release, set `DOMUS_IMAGE`
to the exact registry digest from that release. Do not invent a tag before
publication has completed.

Windows:

```powershell
$env:DOMUS_IMAGE = 'ghcr.io/plagueho/domus-obscura@sha256:<release-digest>'
docker compose pull
docker compose up -d api
docker compose ps
docker compose logs --tail 100 api
```

Linux:

```sh
export DOMUS_IMAGE='ghcr.io/plagueho/domus-obscura@sha256:<release-digest>'
docker compose pull
docker compose up -d api
docker compose ps
docker compose logs --tail 100 api
```

For the locally built image, leave `DOMUS_IMAGE` unset or set it to
`domus-obscura:local`. Open `http://127.0.0.1:8080` after API readiness.
An occupied port or failed startup is an error.

The default starts only the API. Fresh state selects `single_user` anonymous
HTTP, read-only system settings, and disabled providers. A retained configuration
is reused. No failed secure configuration falls back to anonymous mode.

Compose runs without Linux capabilities or privilege elevation and uses a
read-only program filesystem. `/tmp` is a bounded temporary filesystem.
One local named volume retains `/storage`. Fresh setup creates
`/storage/application/config.json` and `/storage/application/state/state.db`. Optional cache, staging and
certificate directories are children of the same application directory.
The API uses `serve --storage-root /storage/application`. The worker uses the same
`/storage/application/config.json`. The extra child keeps the application's setup
parent on the supported volume, not on the container's overlay filesystem.
Existing configuration is operator-managed and read-only
to application components.
Installed browser assets can use the read-only image overlay. This exception
does not apply to state, secrets or selected writable roots. Startup refuses
browser assets on a writable image overlay. Keep `read_only: true`.
Do not use network volume drivers for private state.
The current Linux native adapter requires supported ext-family storage for
private state. A Windows host directory is not a substitute for this volume.

## Mount external media

Set an existing absolute host directory and add `compose.media.yaml`.
The mount is read-only for both services.

Windows:

```powershell
$env:DOMUS_MEDIA_ROOT = 'D:\Media\Music'
docker compose -f compose.yaml -f compose.media.yaml up -d api
```

Linux:

```sh
export DOMUS_MEDIA_ROOT='/srv/media/music'
docker compose -f compose.yaml -f compose.media.yaml up -d api
```

The same data appears at `/media` inside the container.
Use `/media` in application configuration, not the host path.
The override sets `create_host_path: false`. Acceptance requires missing source
directories to fail without creation. The tested Docker Desktop host did not
meet that requirement. Do not treat this option as verified protection.
See [Docker bind mounts](https://docs.docker.com/engine/storage/bind-mounts/).

Run the generated mount checks on the selected host/filesystem before operating
an existing library. Windows path translation is not proof of junction safety,
stable file identities, or private Unix permissions.
Do not use Windows bind mounts for SQLite, credentials, TLS private keys, or
cache/staging state. Keep those resources in Linux-private local volumes.

Mounting data does not select, register or approve it. Stop all state users.
Inspect an existing candidate with the runtime binary:

```sh
docker compose -f compose.yaml -f compose.media.yaml run --rm --no-deps api \
  storage-root-inspect /media media main 1 media ""
```

Review the candidate and the combined operator selection before editing
`/storage/application/config.json`. Follow [additional-root selection](storage.md#inspecting-an-additional-root)
for provider mapping, complete expectations and exact topology assurances where
required. Assurances are trusted operator statements, not measured separation.
Never generate them automatically for arbitrary operator mounts.

Run `configuration-check /storage/application/config.json` with all state users stopped.
Then register a separately approved new library:

```sh
docker compose -f compose.yaml -f compose.media.yaml run --rm --no-deps api \
  location-add --host /storage/application/config.json /media main
```

The host policy must permit this command. Use the returned location ID with the
existing guarded scan and browse commands. Bare database-path commands are not
supported. Existing Windows-native bindings are not Linux-container bindings.
Do not copy database bindings, configuration, owner markers or receipts to
authorize replacement storage. Incompatible pre-production state needs an
explicit fresh setup, not migration, adoption or reset. Preserve existing data.

Grant write access only for a separately approved media operation.
Use a reviewed Compose override for only that mount and service.
Never recursively change permissions on an existing library to resolve a
container access error. No container startup grants source, plan, or artwork
approval.

## Configure the worker and secure mode

Stop all containers that use the state before manual configuration editing:

```sh
docker compose --profile worker stop
```

The operator can edit private JSON under the same `1000:1000` runtime identity. Preserve mode
`0600` and its stable sibling lock. Do not replace a live lock.
Keep the configuration inside an admitted private storage boundary. The
application never saves changes to existing host JSON. The
`system_settings_read_only` policy controls SQLite commands, not host JSON writes.
Do not enable the removed configuration-save or API-only reload operations.

An explicit `production_worker` section enables the worker.
A scan-only worker can use:

```json
{
  "production_worker": {
    "queue_capacity": 64,
    "acquisition": null
  }
}
```

Preserve all other configuration sections. Do not substitute this fragment for
the complete saved configuration.
Use `system_settings_read_only: false` only when administrator configuration
writes and scan commands are intended.

Start the API and the explicitly configured worker:

```sh
docker compose --profile worker up -d
```

Compose waits for the API's browser readiness before worker startup.
Inspect `/api/local/status` for an online worker and its exact active
configuration revision. A running worker container is not readiness evidence.
Use the same media override files for both startup and later commands.
Do not scale the worker or run another deployment against the same state.

Production still requires Entra and validated TLS material.
Explicit development still requires trusted loopback HTTPS.
Provide private material through Linux-private file references.
For HTTPS or a changed port, override the API health environment with the
configured browser URL and, where required, `DOMUS_HEALTH_CA`.
The CA path identifies a public trust certificate, not a private key.
The probe does not disable certificate validation or follow redirects.
Generated health tests check HTTP 200, redirect rejection, and HTTPS CA trust.

Operator edits do not activate a new configuration in running processes.
Stop all state users, edit and validate, then restart all with the same file.
Review pending work and use the existing safe boundary.
Provider credentials, plaintext-login acknowledgement, peer reachability, and
live transfers remain separately configured and approved.

## Stop, backup, and upgrade

```sh
docker compose --profile worker stop
docker compose --profile worker down
```

Stop gives the application up to two minutes to drain. A forced exit preserves
recovery evidence; it does not approve deletion or reset uncertain submissions.
Use the [operator guide](operator-guide.md) for recovery and consistent backups.

Do not use `docker compose down --volumes`. That deletes retained state.
Stop every state user before backup or restore. Copy the complete private state
and required bound roots with the existing stopped-process procedure.

Retain the exact image digest used by a deployment. Acceptance requires the same image and mount
layout to preserve native bindings after recreation.
A new image or Docker/filesystem change can change storage admission.
The operator-selected logical bindings exclude container-specific ancestors,
but retain complete expectations within the selected boundary.
Do not automatically rewrite bindings after an upgrade.
Test the candidate against generated retained state first. If binding checks
fail, preserve the original deployment and use explicit reviewed recovery.
Do not report an unavailable mount as confirmed absence.

## Release and evidence

The Docker workflow builds and checks Linux candidates without publication on
pull requests. A release requires the same candidate to pass on a separately
approved Windows Docker Desktop runner. The Windows job does not install Docker,
enable host networking, or register a runner.

Only the publish job receives `packages: write`.
Use a protected `docker-release` environment for release approval.
The publish job fails if that environment has no required reviewers.
The release version must match the package version.
Both reports must identify the same image and clean source revision.
The report keeps the engine-local image ID for diagnostics. Cross-host comparison
uses the digest of the saved image configuration, which includes the layer-content
identities. Engine-local IDs can differ between image storage backends.
Public GHCR packages permit anonymous pulls, but new packages start private.
An operator must approve and perform the initial public-visibility change.
Verify an anonymous pull before claiming a public release.
See [GitHub Container Registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).

Dependency notice collection is not independent legal review.
Keep the existing distribution review in #26 separate.
Keep built, checked on Linux, checked on Windows, published, and publicly
pullable states separate. A missing host result blocks issue #66 acceptance.
