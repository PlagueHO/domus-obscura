# Real application developer commands

These commands use the built connected frontend and production API/worker.
They do not use `frontend/local-test`. The current foundation is not complete
V1 acceptance or an installed service.

Run Windows tasks through PowerShell 7 and `dev.ps1`. Run Linux tasks through
`cargo task`. `node scripts/developer-command.mjs <task>` selects the same launcher
for Copilot app shortcuts. Use the repository tool pins. Missing tools are errors,
not permission to install them.

## Select native persistent state

Keep machine-specific selectors in your local shell environment, outside Git.
The default configuration for these tasks is
`DOMUS_DEV_ROOT\application\config.json`. `DOMUS_DEV_CONFIG` overrides that file.
You can also pass one absolute configuration path to inspection and lifecycle tasks.

```powershell
$env:DOMUS_DEV_ROOT = 'D:\DomusDev'
.\dev.ps1 dev-doctor
.\dev.ps1 dev-build
.\dev.ps1 dev-inspect
```

On Linux, select an existing private local parent:

```sh
export DOMUS_DEV_ROOT="$HOME/domus-development"
cargo task dev-doctor
cargo task dev-build
cargo task dev-inspect
```

`dev-inspect` returns redacted configuration and passive validation. It does not
create state, migrate SQLite, bind roots, contact providers, or open listeners.
Failure is not an invitation to reset state. Passive validation does not prove
startup or present library availability.

### Existing `D:\DomusDev` evidence

Read-only inspection for #73 found legacy `library`, `staging`, `state`, and
configuration files. Both `service.local.json` and `service.download.json` lack
the current storage selection. The latter enables `local_acquisition`.
No database compatibility, certificate validity, or root binding was proved.
The legacy application was not started.

The selected development path is a new `D:\DomusDev\application` and a new
`D:\DomusDev\developer-library`. Old state, media, credentials, and configuration
stay unchanged. No existing roots are adopted. Do not copy bindings or credentials
to make a new setup look like the old one.

After separate approval to create these fresh targets:

```powershell
.\dev.ps1 dev-setup --confirm-fresh
```

Windows setup refuses either occupied target. It creates private current-user
application storage, a fresh generated empty library, staging, editable shared
system policy, and a scan-only worker. Providers, downloads, and sharing stay
disabled. This task uses explicit `single_user` HTTP on `127.0.0.1:8080`.
It does not change certificate trust.

For a different port, or fresh optional developer HTTPS, use the existing wrapper:

```powershell
.\scripts\local-setup.ps1 -Directory D:\DomusDev -UseExistingParent -SingleUserHttp -Port 8081
.\scripts\local-setup.ps1 -Directory C:\NewDomusHttps -Port 8443
```

These are alternatives, not commands to run on an existing setup. HTTPS creates
a fresh 90-day self-signed certificate without trusting it. Browser trust remains
a separate operator action. Do not disable certificate checks.

Linux `dev-setup --confirm-fresh` creates fresh `application` storage with a
scan-only worker and editable policy. It does not register an arbitrary media
root. Select and bind additional generated media through the existing
[storage procedure](storage.md#inspecting-an-additional-root) after review.
For existing developer certificates, use
[manual HTTPS setup](local-development.md#manual-setup-on-linux-or-with-an-existing-developer-certificate).

`single_user` means anonymous HTTP on direct loopback. Explicit `development`
means anonymous HTTPS with valid TLS. Production still requires Entra and TLS.
A failed secure configuration never falls back to anonymous HTTP.

## Native lifecycle

```powershell
.\dev.ps1 dev-run
.\dev.ps1 dev-status
.\dev.ps1 dev-logs
.\dev.ps1 dev-stop
.\dev.ps1 dev-restart
```

Use the same task names with `cargo task` on Linux. Run status, logs, stop, or
restart in a second terminal. `dev-run` and `dev-restart` remain in the foreground.
Stop requests are accepted during readiness checks and drain any children already
started.
Copilot opens the URL only after the launcher emits `Domus ready: <actual-origin>`.
Readiness checks the exact built index bytes, anonymous session, active API
configuration revision, and configured worker heartbeat. A running process is
not sufficient.

The private supervisor starts separate API and configured worker processes.
An embedded `local_acquisition` configuration does not also start a production
worker. Duplicate supervisor ownership and occupied ports fail. Control uses a
current-user Windows named pipe or private Unix socket, not an HTTP endpoint.
No PID file is used as authority. Application children do not inherit Copilot
GitHub credential variables.

Stop drains the worker first, then the API. Ctrl+C requests the same drain.
Restart waits for confirmed shutdown before starting with the saved configuration.
A control timeout leaves shutdown unconfirmed; do not start another instance.
No routine force kill is sent. Review durable intent and receipts after an
unexpected exit. Native logs retain only the last 100 bounded lines in supervisor
memory; they are not a durable job history. Oversized lines produce a diagnostic
and are discarded without closing the child's output pipe. Disconnected or
unresponsive status/log clients do not trigger application shutdown. Control
replies have a five-second timeout.
After a crash, inspect an occupied
Unix control socket and verify that its owner has stopped before explicit cleanup.
Never delete stable application lock files.

Stop all state users before operator edits to host JSON. Other host settings
remain operator-managed. Confirmed library folder and Soulseek account changes
in Settings are narrow exceptions. They require the native supervisor.
System policy commands remain gated by the current read-only flag.
Use the [operator maintenance boundary](operator-guide.md#stop-and-exclude-all-state-users)
for migrations, backups, recovery, and external configuration changes.

### Configure library folders in Settings

Open **System settings > Library folders**. Select **Change…** under **Primary
library** or **Add backup** under **Backup libraries**. Set a primary library
before you add a backup.
Select **Browse…** to choose a local drive and navigate directories.
You can also enter an existing absolute folder path on the application host.
The dialog reads through Rust, not through a browser-local file picker. It
blocks links, reparse points and known private application storage. **Use this
folder** only fills the path; it does not approve or register the folder.
Directory pages scan at most 128 entries. The dialog holds at most 2048 displayed
folders. Enter a more specific path when that limit is reached. Closing the
dialog releases its continuation. Listings expire after three minutes and are
discarded on the next browser request. Changed listings must be reloaded.
Listings from another server instance cannot be continued.

Use `.\dev.ps1 -TaskArgs @('browser','folder-picker.spec.ts')` on Windows to
check this dialog with generated browser fixtures. On Linux, use
`cargo task browser folder-picker.spec.ts`. The browser task forwards selectors
to Playwright and keeps the private fixture asset setup.

For Windows SMB, enter a UNC path such as `\\server\Music`. For Linux SMB or
NFS, enter a mounted path such as `/mnt/music`. The account that runs the Rust
API must have read access. If opening a share fails, check the exact path,
host connectivity and that account's access. The picker does not discover
shares, save SMB/NFS credentials or mount shares. A successful browse does
not approve the path for library operations.

The folder review marks a recognized SMB/NFS filesystem as **scan only**.
The host must provide the required directory identities.
An unsupported identity or changed binding fails without treating files as missing.
Scan-only folders cannot be acquisition or recovery destinations. A scan-only
primary prevents album acquisition because every plan requires a writable primary.

Select **Check** to verify the folder. For a backup, review the storage layout
and confirm that none of the other folders is another path to the same folder
or contains it. The check rejects known overlaps and duplicate backups.

Select **Save primary library change** or **Save backup library change** to
record approval for that exact folder, role and configuration revision. This
does not activate the location. You can cancel the saved change before
activation starts. Select **Restart and apply primary library** or **Restart
and apply backup library** to stop the worker and API at safe boundaries,
recheck the location and unfinished work, publish the selected storage changes,
and restart both processes.
The browser checks the saved activation result after reconnecting.

No media is moved, copied, replaced, or deleted. The previous primary retains
its location ID, native binding, index, plans, and receipts for history and
recovery. It is not made a backup. Normal library lists and new plans use only
current folders. A newly registered primary has unknown scan coverage.
Adding a backup leaves the primary, existing backups and scan schedule unchanged.
Adding a folder does not queue a scan. Use **Library scans** to update the index.

Unfinished jobs, worker requests, scans, and recovery operations block a change.
A failed recheck before configuration publication leaves the configuration
unchanged. Uncertain publication or activation leaves the application stopped;
inspect the retained change and `.domus-primary-<id>.pending` evidence before
restarting. Do not remove stable locks or infer success from a lost HTTP response.
Startup can finish only the exact published, approved activation.

This flow requires the native application supervisor. Paired media, Docker and
installed services require their separate operator procedure. You can still
register an eligible folder that the operator already selected on the host.

## Separate Docker persistent state

Docker uses a local Linux volume, not the native Windows SQLite directory.
Windows bind-mounted state, secrets, cache, and staging are not supported.
Use Linux Engine or Docker Desktop in Linux-container mode with supported host
networking. Docker Desktop requires 4.34 or later and an explicitly enabled
host-network feature. Settings changes need separate approval.

Select one explicit persistent deployment name. Do not reuse another project's
name. A reviewed `DOMUS_DEV_COMPOSE_OVERRIDE` can select an additional absolute
Compose file for approved media/pairing and health configuration.

```powershell
$env:DOMUS_DEV_DOCKER_PROJECT = 'domus-development'
$env:DOMUS_IMAGE = 'domus-obscura:local'
.\dev.ps1 dev-docker-doctor
.\dev.ps1 docker-build
.\dev.ps1 dev-docker-setup --confirm-fresh
.\dev.ps1 dev-docker-run
.\dev.ps1 dev-docker-status
.\dev.ps1 dev-docker-logs
.\dev.ps1 dev-docker-stop
.\dev.ps1 dev-docker-restart
```

On Linux, export the same selectors and use `cargo task <task>`.
The selected named volume retains `/storage/application/config.json` and
`/storage/application/state/state.db`. Setup is explicit and refuses retained
configuration. Lifecycle tasks never remove volumes. Build/check uses `uv` and
installed Python 3.14 without automatic Python downloads.
`docker-build` and `docker-check` use `DOMUS_IMAGE` unless an explicit `--image`
argument overrides it. They forward verification arguments to the existing
Docker checker without shell interpolation.

Run validates stopped configuration, waits for the API and configured worker,
then verifies the actual host browser response and exact configuration revision.
Status also checks host readiness. Logs are bounded to the last 100 lines.
Stop sends SIGTERM to exact selected container IDs and waits for normal exit.
A drain timeout fails without SIGKILL and prevents restart. Do not substitute
`docker compose down --volumes`, which deletes retained state.
Failed or unknown container exits also prevent restart until operator inspection.
Setup and lifecycle tasks do not pull images or build them implicitly.

For HTTPS, the selected public CA must also be available to the container health
probe through `DOMUS_HEALTH_CA`. The task derives the actual health URL from saved
configuration. HTTPS probes validate certificates and never follow redirects.
The local native binary must be built for the host readiness check.

Windows media requires the already implemented
[native adapter and paired client](docker.md#configure-the-native-foreground-adapter).
Pairing material, native adapter startup, media registration, and topology
assurances are explicit operator steps. The developer commands do not create or
start an adapter automatically. Missing adapters fail instead of falling back to
guest paths. Linux media uses the documented checked mount layout. Neither mode
copies native database bindings or approves writes to an existing library.

## Manual application test

1. Inspect the selected configuration. Review authentication and startup effects.
2. Start the selected application. Open only its reported loopback URL.
3. Browse saved Library pages. Refresh the index view without starting a scan.
   An empty fresh library is not proof that existing media was indexed.
4. Inspect System settings, the queue, and Decisions. Providers remain disabled
   in a fresh setup. Host configuration is read-only to the application.
5. After approval for a generated-fixture policy change, save one shared setting.
   Stop and restart. Check that the saved value and library binding remain.
6. Stop the application. Confirm normal process/container exit. Retain state.

Catalog search, artwork requests, source discovery, and account login can contact
live providers. Scans change the index. Downloads, publication, replacement,
restore, and recovery deletion have separate approvals and recovery boundaries.
Do not perform them as implicit startup tests.

Use `test developer`, `test backend_configuration`, `check`, `build`, and `smoke`
for native checks. Use `docker-check` for isolated generated-volume checks.
For Windows HTTP and private Linux-volume checks without native media pairing:

```powershell
.\dev.ps1 -TaskArgs @('docker-check', '--windows-private-volume-host', '--report', 'target\docker-development.json')
```

This diagnostic does not mount Windows media. It is not complete Windows
external-media or release acceptance. On Linux, `cargo task docker-check` checks
the native host. `cargo task docker-check --engine-only --volume-media` selects
only the private-volume Engine diagnostic.

The existing connected browser and sample harness tests remain separate evidence.
Report native Windows, native Linux, Docker Engine, and Windows host browser/media
results separately. Missing Docker Engine or host settings block container proof,
not native proof.

## Verified local scope for #73

The separately approved native setup created only the new
`D:\DomusDev\application` and `D:\DomusDev\developer-library` targets.
The real browser bytes, anonymous API identity, and exact worker revision passed.
Native restart retained shared settings and the generated library binding.
Read-only browsing returned an empty library. The queue was empty.
Normal stop ended all owned application processes.

The nine original top-level entries in `D:\DomusDev` retained their attributes,
lengths, and write timestamps. This is metadata evidence, not a recursive byte
audit. No legacy configuration, credentials, or media were adopted.

Docker Desktop ran with its existing settings. The local image build passed its
Linux archive and health checks. The separate `domus-development-73` deployment
used only the `domus-development-73_application` Linux volume.
Windows host readiness, status, logs, restart, and worker-before-API stop passed.
Removing the stopped API/worker containers and creating new containers retained
the exact shared settings, configuration revision, and built browser hash.
The private-volume host diagnostic passed all 14 generated checks and cleanup.

The Unix supervisor also passed two runs inside the Linux guest. Separate
same-volume control containers verified readiness, status, bounded logs, normal
drain, retained settings, and socket cleanup. This is not native Linux-host proof.

The native application and retained Docker API/worker containers are stopped.
Their approved persistent state remains. Temporary diagnostic containers and
volumes were removed. The image was built from uncommitted local source.
These results are developer evidence, not release or complete V1 acceptance.

Rendered browser interaction and actual Copilot shortcut execution remain
unverified. Browser diagnostics found an old running Native Host that did not
match the current Plugin. Restarting Edge is the required browser remediation.
No browser repair, host software installation, certificate trust change, Docker
settings change, native media pairing, provider access, or existing-library write
was performed.
Native Linux-host and external Windows-media acceptance remain separate checks.

## Copilot app shortcuts

Review and accept `.github/github-app.yml` in the app. All scripts are manual.
There are no session creation/archive triggers. The fresh-setup shortcuts still
require prior target review and separate operator approval for their effects.
Local environment selectors must be available to the app's script process.

Configuration and URL detection follow the
[official Copilot repository reference](https://docs.github.com/copilot/reference/github-copilot-app-reference/repository-configuration).
Docker requirements follow the
[official host networking reference](https://docs.docker.com/engine/network/drivers/host/).
