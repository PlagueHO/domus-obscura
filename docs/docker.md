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

The Docker branch includes the completed storage dependency at
`5172b83e0dd755a8caa0c1c9f5ab812ed2654820` from open PR #70. This does not merge
that PR. The PR's rebased head is `cd33a3c880b71ac776b88f845e9071adbcb39700`;
both revisions have the same source tree.
The current image and Docker workflows need their own checks.
Native storage checks are not whole-image Docker acceptance.

The current generated-volume engine workflow passes browser-byte and API-guard
checks, worker scanning, selected bindings, overlap refusal, recreation, graceful
stop, forced exit, invalid configuration, and replacement refusal.
The image build also passes the full native archive lifecycle check on a fresh
local volume. These are engine results, not supported-host acceptance.
The current Windows host-loopback retest still fails with connection refused,
although the same fresh API serves its session inside the engine.
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
