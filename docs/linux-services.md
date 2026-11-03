# Linux package and systemd services

## Supported candidate

The first target is `x86_64-unknown-linux-gnu` on Ubuntu 24.04.
The supported baseline is glibc 2.39 and systemd 255.
Older distributions, ARM64, and musl are not supported by this package task.
See the official [glibc package](https://packages.ubuntu.com/noble/libc6)
and [systemd package](https://packages.ubuntu.com/noble/systemd).

The candidate contains `domus`, sibling `browser` assets, migrations,
systemd units, this guide, a package checker, notices, and build provenance.
Native JPEG and SQLite are linked into the executable.
Node.js is a build tool, not an application runtime.
The package requires the documented GNU/Linux OS libraries and loader.
It does not include glibc or the host systemd installation.
ELF inspection rejects unknown libraries, external runtime search paths,
and glibc symbol requirements above the supported baseline.

The manifest records exact file hashes and modes, ELF requirements, tool
versions, source revision, and dirty state. Hashes detect changes; they do not
authenticate a publisher. Obtain the candidate from a trusted build.
Notice collection keeps exact resolved source texts, the file-id patch and
both licenses, certificate provenance, and native JPEG notices.
Missing notices stop packaging. Independent distribution review remains #26.

## Build and check

Run on the native Ubuntu 24.04 baseline with the repository tool pins:

```sh
cargo task bootstrap
cargo task check
cargo task package-linux
cargo task package-check-linux
```

Rust, Node.js, pnpm, CMake, and a C compiler are build prerequisites.
The package checks also use Python 3, GNU tar, readelf, and systemd-analyze.
Python is an operator/check tool, not a dependency of the application process.
Do not install missing host tools without approval.

In WSL, use a native Linux checkout on Linux disk for package validation.
A Windows-mounted worktree can report unsuitable Unix permissions and contain
Windows-only Git references. Windows results are not native Linux evidence.

Output paths are `target/linux-package` and
`target/domus-obscura-0.1.0-linux-x86_64.tar.gz`.
The release build explicitly uses `target` even when `CARGO_TARGET_DIR` is set,
so packaging cannot select an older executable from a different build path.
The archive has a sibling `.sha256` file.
Package creation checks the archive in fresh temporary state before replacing
previous verified output. Changed or unrelated files block replacement.
Do not store application data in generated package directories.

The archive checker bounds file count, depth, individual size, and expanded
size. It rejects traversal, links, special files, duplicate entries, unsafe
modes, missing files, and changed hashes.
The entry limit includes the archive root header. Directory headers must not
contain data. A duplicate root header or unsafe root mode is an error.
The process check serves the real browser from an extracted payload outside
the checkout with a system-only PATH. It uses loopback HTTP and no providers.
It checks notifications, graceful stop, forced exit, worker contention,
API restart under a worker, retained SQLite state, and rejected bad inputs.
It does not install services or test a real download.

## Foreground use

Extract the trusted archive into a new empty directory as a normal user.
Check the extracted directory with the packaged checker:

```sh
python3 package-check.py --verify-installed /absolute/extracted/package
./domus serve
```

Open `http://127.0.0.1:8080`.
No configuration starts explicit single-user mode with providers disabled.
Initial state uses `XDG_DATA_HOME`, or `$HOME/.local/share` when unset.
Keep it on local Linux disk, not a Windows WSL mount or network filesystem.
The real browser files must remain beside `domus`.

For configured operation, use the same account and configuration for both
foreground processes:

```sh
./domus serve /absolute/private/config/service.json
./domus worker /absolute/private/config/service.json
```

Explicit configuration never falls back after failure.
Production requires Entra. Explicit development retains loopback HTTPS.
Do not weaken authentication, Origin, peer, TLS, or CSRF guards.
Ctrl+C and SIGTERM stop through the existing drain path.
Do not run a foreground worker against state owned by a service worker.
The worker does not migrate SQLite.
On Linux, API startup holds the stable worker lock during migration.
A live worker permits only a current-schema API restart.

## Manual service setup

Installation requires separate approval for the selected host.
The package does not install itself, create accounts, grant permissions,
register services, start downloads, or change the firewall.
Never run `domus` as root. The explicit systemd entry point rejects root.

Use one existing dedicated non-root account and group for both services.
The supplied units use `domus-obscura`.
Do not use two accounts with shared-write groups to bypass private-file rules.
Use reviewed drop-ins if the approved account has a different name.
The application private-file policy also trusts the host administrator.

Use these paths:

| Resource                              | Default location                | Required access                                                          |
| ------------------------------------- | ------------------------------- | ------------------------------------------------------------------------ |
| Program payload                       | `/opt/domus-obscura`            | Root-owned; runtime read/execute only                                    |
| SQLite, WAL/journal, stable locks     | `/var/lib/domus-obscura`        | Runtime owner; local disk; private read/write                            |
| Backend JSON and stable save lock     | `/var/lib/domus-obscura/config` | Same owner; private directory; JSON mode `0600`                          |
| Credential and private-key references | Approved private directories    | Same owner; private files; no group or other access                      |
| Certificate journals and generations  | Approved local state            | Same owner; required lifecycle write access                              |
| Bound cache and staging               | Approved local state/roots      | Same owner; private access; retained native bindings                     |
| Media roots                           | Approved mounts outside state   | Read access for scans; explicit write access for approved worker effects |

Create only approved new directories. Do not recursively change permissions
on an existing library. Set `umask 077` before initial state creation.
Prepare SQLite explicitly with `domus init` under
the runtime account with all state users stopped. This is initial setup,
not permission to run concurrent migration. Use absolute Linux paths.
Prepare and validate the backend JSON separately. Add `production_worker`
only when its operations are intended. Its smallest section is:

```json
{
  "production_worker": {
    "queue_capacity": 64,
    "acquisition": null
  }
}
```

Keep configuration private even when it contains only file references.
With editable settings, the JSON parent must permit its stable sibling lock
and atomic saves. With `system_settings_read_only`, application commands still
enforce the policy. Saving JSON does not activate a worker configuration.

Copy only a verified payload into a new empty root-owned program directory.
Check it again with `package-check.py --verify-installed`.
Do not overlay an existing package or an unknown directory.
Copy the two unchanged supplied units into `/etc/systemd/system`.
Then reload unit definitions:

```sh
sudo systemctl daemon-reload
```

These commands are instructions for an approved installation, not actions
performed by package creation.

## Restricted access and mounts

Both units use `Type=notify`, `NotifyAccess=main`, an empty capability set,
`NoNewPrivileges`, private temporary/device namespaces, `ProtectSystem=strict`,
and `ProtectHome=yes`. The default write exception is only
`/var/lib/domus-obscura`. Home-directory media is hidden by default.
Do not disable the restrictions to make a path work.
Add narrow, reviewed bind/path exceptions for an approved home-root layout.

Add site-specific drop-ins for external mounts. For example, an approved
publication worker can have:

```ini
[Unit]
RequiresMountsFor=/srv/media/main /srv/media/backup

[Service]
ReadWritePaths=/srv/media/main /srv/media/backup
```

Do not grant this write access to a scan-only deployment.
Add required mount ordering to each service that reads those mounts.
Add separate write exceptions for approved cache/staging locations outside
the default state directory. Private credential directories can have
`ReadOnlyPaths` exceptions inside the writable state tree.
Keep managed certificate journal locations writable.

Check access under the actual runtime account and service restrictions.
An administrator's successful read is not runtime permission evidence.
Some systemd restrictions depend on kernel namespaces and seccomp support.
See [systemd.exec](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html).
Unit syntax checks do not prove that host restrictions are active.

An unavailable mount or changed native binding must produce an error.
Do not create a replacement empty root, rebind a copied marker, or report
an incomplete scan as confirmed absence. Inspect retained recovery before
resuming approved work.
Native identity comparisons do not prove protection against identity reuse or
filesystem cloning. Linux can reuse device/inode pairs, including after tmpfs
replacement. A changed-identity remount check must verify that the replacement
actually has a different native identity.

## Start, stop, and status

```sh
sudo systemctl start domus-obscura-api.service
sudo systemctl start domus-obscura-worker.service
systemctl status domus-obscura-api.service domus-obscura-worker.service
sudo systemctl stop domus-obscura-worker.service
sudo systemctl stop domus-obscura-api.service
```

API readiness follows initialization and listener startup.
Worker startup waits for the API start job and explicitly requires an active
API before launching. `Wants` and `After` do not couple the running worker
to later API restarts. The worker can drain independently.
Running is readiness evidence, not successful acquisition evidence.

Startup has a 120-second budget. Stop has a 180-second budget.
SIGTERM requests drain. Native locks stay held through operation completion.
`KillMode=mixed` sends the initial stop to the main process and permits
SIGKILL for remaining processes after the stop budget.
Forced termination preserves committed intent and recovery evidence.
Fenced leases still control recovery. A restart never approves work, answers
questions, resets uncertain transfer intent, or retries failed jobs.

Failure restarts wait 30 seconds. The units allow three starts in 300 seconds,
including the first start, then stop retrying.
A manual stop does not trigger `Restart=on-failure`.
Inspect configuration, permissions, mount identity, and durable recovery
before clearing a failed start limit with `systemctl reset-failed`.
See [systemd.service](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html).

Enable boot startup only after both services pass the approved installed check:

```sh
sudo systemctl enable domus-obscura-api.service domus-obscura-worker.service
```

Use `journalctl -u domus-obscura-api.service` and the corresponding worker
command for startup errors. Keep credentials and private inputs out of logs.
Service restart does not start a new download without durable approved work.

## Update and consistent backup

1. Disable boot startup for both units. Apply runtime masks during maintenance
   so a dependency or another systemd start cannot restart them.
2. Stop worker, then API. Stop all foreground state users. Confirm stopped
   status and coordinate with other operators.
3. Back up the complete SQLite state with all users stopped, including retained
   WAL/journal files. Do not copy a live database file alone.
4. Preserve configuration, credential references, native bindings, certificate
   state, cache/staging ownership, receipts, and recovery journals.
   Keep backups private. Check a separate isolated backup without live roots
   or providers. Wider backup/restore procedures remain #19.
5. Verify the new archive separately. Verify the old installed manifest.
   A changed or unknown program file blocks payload removal.
6. Remove only the old verified payload files with the checker.
   Keep the root directory. Copy the new verified payload into the empty root.
   Replace only unchanged owned unit files. Keep site-specific drop-ins.
7. Reload unit definitions. Remove the runtime masks. Start API, then worker.
   Only API startup may migrate the existing database.
   Inspect recovery and active configuration before enabling boot startup again.

Use `systemctl mask --runtime` and `systemctl unmask --runtime` with the exact
two unit names. Masks do not prevent a foreground process; coordinate all users.
After a schema upgrade, an older binary is not a database rollback.
Restore only a tested consistent backup under an explicit recovery procedure.
No update command replaces configuration or deletes application data.

## Non-destructive uninstall

Disable and stop both units. Confirm stopped state and no foreground users.
Compare each installed unit with its package-owned copy before removing it.
If a unit changed, preserve it and report incomplete removal.
Remove only the two unchanged files in `/etc/systemd/system`.
Reload unit definitions. Preserve all operator-authored drop-ins.

Before payload removal, run:

```sh
python3 /opt/domus-obscura/package-check.py --verify-installed /opt/domus-obscura
python3 /opt/domus-obscura/package-check.py --remove-installed /opt/domus-obscura
```

Program-file removal requires the appropriate owner/administrator permission.
The checker does not stop services or grant that permission.
It checks all files before deleting any. It then rechecks each listed file
through no-follow parent/file handles. Changed files cause an explicit error.
It never recursively removes the root or adds unknown files to its deletion
list. A concurrent change can leave removal incomplete; inspect the error.
Keep the verified archive for recovery.

Accounts, permissions, configuration, secrets, certificates, SQLite,
cache/staging roots, media, and recovery records remain unchanged.
Do not remove them as part of uninstall.

## Acceptance boundary

Keep these evidence classes separate:

| Check               | Required evidence                                                                                |
| ------------------- | ------------------------------------------------------------------------------------------------ |
| Local source checks | Rust/package/helper tests and full check results for these changes                               |
| WSL checks          | Native Linux helper/process results; not Windows results                                         |
| Linux CI package    | Exact revision, baseline, ELF inspection, extracted archive and process checks                   |
| Installed systemd   | Actual non-root account, readiness ordering, restart limits, permissions and active restrictions |
| Host lifecycle      | Cold boot, reboot, graceful/forced systemd stop, offline/remounted drives, lease recovery        |

Installed checks need separate approval. Use generated roots and offline
fixtures where possible. Real acquisition, live Azure/CA operations,
existing-library writes, and public exposure remain separate approvals.
Do not close #18 or claim complete service acceptance when required installed
or reboot checks have not run.

## Reproducible service acceptance

The source tree contains an opt-in runner:
`packaging/linux/service-acceptance.py`.
It is not an installer and is not included in the application archive.
It does not create accounts, install software, register units, mount drives,
change permissions, or reboot. Its `run` and `cleanup` modes use
non-interactive `sudo` for the exact isolated test units. Review and approve
these effects for the selected host before use. Do not grant general
passwordless sudo to make a test work.

The default evidence class for manual host checks is `not_run`.
A successful runner command does not close #18.
The runner records the package manifest hash, source revision/dirty state,
host/kernel markers, runtime UID, rendered units, and individual results.
It excludes credentials and raw application journals.
The isolated units keep the shipped sandbox and timeout values.
Only unit names, the existing account/group, program path, configuration path,
and writable state path change. These results do not prove installation at
the production paths or permission to access an existing library.

Unprivileged checks run without installing services:

```sh
python3 packaging/linux/service-acceptance-tests.py
cargo task test service_acceptance
```

The Rust checks use fresh state and generated media. They prepare artwork
approval, plan approval, transfer receipts, completed normalization, and durable
worker requests through existing application rules. A test-only SQLite trigger
interrupts publication receipt persistence after intent and file creation.
The trigger is removed before export. The fixtures do not invent approval or
receipt rows. The worker resumes local publication without a provider login.
The helper is a test binary, not a production command or HTTP endpoint.

### Prepare an approved isolated run

Use Ubuntu 24.04 on native Linux disk. Use an existing non-root runtime account.
Reserve port 8080 for the loopback test API.
Keep the verified program under a new administrator-owned path outside the
writable fixture root, for example `/opt/domus-acceptance-trial-01`.
All program files and parent directories must be administrator-owned and must
not permit group/other writes.

Select a new identifier for each case. The root must be exactly
`/var/lib/domus-acceptance-<identifier>`.
An administrator must separately approve and create that new empty directory
for the runtime account with mode `0700`. Do not reuse a service data directory.
The following example assumes that this setup and package staging are approved
and complete:

```sh
RUN=trial-01
ROOT=/var/lib/domus-acceptance-$RUN
PACKAGE=/opt/domus-acceptance-trial-01
DOMUS_SERVICE_ACCEPTANCE_ROOT="$ROOT" DOMUS_SERVICE_ACCEPTANCE_PACKAGE="$PACKAGE" \
  cargo task test --release \
  journal::service_acceptance::service_acceptance_export -- --ignored --exact
python3 packaging/linux/service-acceptance.py preflight \
  --run-id "$RUN" --root "$ROOT" --package "$PACKAGE"
python3 packaging/linux/service-acceptance.py prepare \
  --run-id "$RUN" --root "$ROOT" --package "$PACKAGE"
```

Use the release test profile for this large export. The normal unprivileged
regressions use small fixtures.
The exporter writes only inside the empty selected root. It creates 8192 scan
directories and three generated MP3 inputs of about 33 MiB each, plus normalized
outputs and private state. Allow space for both normalized and published copies.
The generated credentials are not live credentials.
The exporter validates the selected package's browser assets and records their
absolute path in the explicit API configuration. The runner requires that exact
path. Missing or different browser assets are not a successful installed check.
The configured Soulseek address is an unused validation input: only scan and
local `Publish` requests are queued. Do not submit acquisition work to this fixture.
No sharing account is enabled.
Discovery, metadata, artwork, and artist-image providers are disabled.
The runner rejects changed configuration, live credentials, and any request
other than the two generated scan/publication requests before service startup.

`preflight` only reads existing inputs and prints its report.
It does not migrate or open SQLite, create files, escalate privileges, or start
a stopped WSL distribution. `prepare` only renders unit files under `$ROOT/units`.
Neither command installs or starts services.
The package checker must be from the same trusted verified package.

Review the two rendered units. After separate installation approval, copy only
those two files to `/etc/systemd/system` as root-owned files with mode `0644`.
Do not overwrite an existing unit. Run `systemctl daemon-reload` after the copy.
Do not add test drop-ins. The runner rejects changed unit files and drop-ins.

### Run and collect

Run as the runtime account. The host argument is an explicit execution gate,
not a substitute for host approval:

```sh
python3 packaging/linux/service-acceptance.py run \
  --run-id "$RUN" --root "$ROOT" --package "$PACKAGE" \
  --approved-host "$(hostname)" --case lifecycle
```

`lifecycle` starts only the worker unit so the manager must start the API
dependency first.
It checks readiness ordering, real browser delivery, the effective
non-root UID, zero effective capabilities, no-new-privileges, a separate mount
namespace, a read-only root mount, API restart without worker replacement,
generated work completion, output hashes, retained approvals/receipts,
SQLite identity/integrity, and graceful stop.
It does not infer every private-path access result from these sandbox checks.

Use a new fixture root and new units for each `--case forced` and
`--case graceful` run. Both cases wait for committed progress in the scan and
publication lanes. They stop the owned worker briefly with SIGSTOP and confirm
that both operations remain active. A competing foreground worker must fail
with the native ownership error.
If the work finishes before the observation, the check fails as inconclusive.
Do not report idle-process termination as active recovery evidence.

The forced case sends SIGKILL, cancels the pending restart, and starts the same
worker again. It checks request generations before normal lease expiry and
waits for recovery without editing lease timestamps.
The graceful case asks systemd to stop the frozen worker; systemd resumes it
for the normal SIGTERM drain. Both cases then require final file hashes,
complete scan coverage, unchanged approvals/normalization receipts, and retained
database identity. All waits have bounds.
This tests local publication recovery, not uncertain network transfer recovery.
Run the existing worker, queue, transfer, publication, and recovery Rust tests
for stale claims, partial outputs, and failed/waiting policy separately.

`collect` reads current unit status and durable state without manager mutations:

```sh
python3 packaging/linux/service-acceptance.py collect \
  --run-id "$RUN" --root "$ROOT" --package "$PACKAGE"
```

Reports are new private `report-*.json` files in the fixture root.
Failed runs save a failed result before returning an error.
A failed run leaves state and services for inspection; it does not hide the
failure with automatic deletion. An observed SIGSTOP is released on error.
Use approved cleanup after inspection.

### Host checklist and evidence still required

These are separate manual cases. Do not replace them with a passed unit syntax
check, a mocked runner test, or a historical WSL result.

| Case                            | Required action and evidence                                                                                                                                                                                                                                                                                                                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Actual stop timeout             | On isolated units, use an approved debugger to keep only the owned application stopped through manager signals. Record the unchanged 180-second stop budget, manager timeout result, SIGKILL escalation, retained SQLite integrity, and ownership recovery. SIGSTOP alone is insufficient because systemd sends SIGCONT during stop.                         |
| Restart rate limit              | With all state users stopped, make only the generated private configuration invalid. Record exactly three failed application starts within 300 seconds, followed by manager rate-limit rejection. Preserve the journal excerpt for these units only. Restore the exact generated input before an explicit reset/start.                                       |
| Private access                  | Test the generated JSON, stable save lock, dummy credential, cache/staging bindings, and media roots under the runtime UID and actual service mount namespace. Record permitted access and denied group/other or unavailable-path cases. Do not use an administrator read or configured directives as proof.                                                 |
| Mount loss                      | With separate mount approval, use a generated filesystem containing only fixture data. Record scan failure, retained indexed entries and bindings, incomplete coverage, and absence of replacement content after loss. Restore the mount before recovery. Never unmount an existing library.                                                                 |
| Changed identity                | Record device/inode pairs before and after the approved generated remount. Reject the test as inconclusive if the pair is reused. Confirm rejection of a genuinely different identity without rebinding a copied marker. This does not resolve identity reuse or cloning.                                                                                    |
| Update/removal                  | With all fixture state users stopped, hash configuration, dummy credentials, database/WAL, binding markers, media, receipts/recovery, and operator drop-ins. Use the existing checker on generated program copies. Check refusal for changed/unknown program files, then exact owned-file removal. Compare the retained inputs. Do not use production files. |
| Failed/waiting and stale claims | Retain focused Rust workflow results separately. An installed-service assertion needs a generated failed/waiting request or old claim and evidence that restart neither approves nor retries it. A completed happy-path request is not proof.                                                                                                                |

For boot checks, first complete the installed cases.
Separately approve enablement of only the two isolated units.
Capture a `boot-before` report. The operator then performs the approved restart,
physical reboot, or cold boot without a manual service start.
Capture `boot-after` with `--before /absolute/path/to/report-boot-before-....json`.
Both modes take the same run/root/package arguments.
The reports compare kernel boot ID, PID 1 start marker, and database identity.
They also check running services, effective restrictions, and real browser access.
The runner never initiates a reboot or enables units.
Its result is only `running_after_observed_restart`: the operator must retain
separate evidence of automatic startup and the physical boot method.
A WSL manager restart cannot satisfy physical boot or reboot acceptance.

### Exact-resource cleanup

After review, use separately approved cleanup:

```sh
python3 packaging/linux/service-acceptance.py cleanup \
  --run-id "$RUN" --root "$ROOT" --package "$PACKAGE" \
  --approved-host "$(hostname)"
```

Cleanup checks the unit source, exact content, ownership, and absence of drop-ins.
It stops and disables only these two units, rechecks them, unlinks only their
unchanged files, and reloads the manager. A changed input causes an error.
Partial cleanup is possible; inspect the error and retained report.
It does not delete program files, fixture data, accounts, mounts, or evidence.
Confirm that the two unit registrations and enable links are gone.
Keep the fixture root for recovery inspection. Its removal needs a separate,
exact-path operator decision after evidence retention.
