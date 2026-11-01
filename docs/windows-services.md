# Windows installation and services

## Clone and run first

Services and the installer are optional. From a clone with the pinned build
prerequisites installed, run:

```powershell
.\dev.ps1 bootstrap
.\dev.ps1 dev
```

Open `http://127.0.0.1:8080`. The development task builds the real connected
browser and starts `domus serve` in the foreground. It needs no service account,
service registration, backend configuration, certificate, Azure, or Entra setup.
Ctrl+C uses the normal stop path. Rebuild and restart after frontend edits.

Rust, Node.js, pnpm, MSVC with the Windows SDK, and CMake are build prerequisites.
Use the repository pins. The launcher selects the complete MSVC environment.
Bootstrap restores packages; it does not install system tools.

First launch creates private state under the current user's `LOCALAPPDATA`.
It does not start a worker, enable providers, approve work, or change media.
Existing explicit secure configuration never falls back to anonymous HTTP.
See [local development](local-development.md) for optional configured HTTPS.

## Build a Windows candidate

```powershell
.\dev.ps1 package-windows
.\dev.ps1 package-check
.\dev.ps1 installer-windows
```

Run on Windows x64 with the native MSVC target. Package tasks build the release
executable with static Rust and native JPEG C runtimes. They check PE imports.
Unknown DLL dependencies are errors. Windows system DLLs remain OS prerequisites.
Node.js is not a production runtime.
This includes
[`BCryptPrimitives.dll`](https://learn.microsoft.com/en-us/windows/win32/seccng/processprng)
for the native random-number API. Do not distribute Windows system DLLs.

`target\windows-package` contains the executable, sibling `browser` assets,
migrations, operator helpers, notices, and a hashed manifest. Both file-id
licenses and patch provenance are retained. Dependency notices are collected
from resolved source packages, not license names alone. Independent distribution
review remains #26. Missing source notices block package completion.

Some published dependency archives omit their license files.
`third_party/windows-distribution-dependencies/sources.json` binds supplemental
upstream texts to the exact package version, repository revision, source
directory, and file hashes. The collector rejects a different source.
Do not substitute generic license text or infer missing copyright notices.
The payload no longer depends on `windows-permissions 0.2.4`, whose archive and
recorded revision omitted a source notice. The original `native-file-security`
boundary now reads and copies descriptors through borrowed handles using the
Windows SDK. `winsafe` resolves SID aliases. The existing SDDL allow-list remains
the permission policy. This replacement does not complete distribution review.

Package rebuild stages and checks the new payload before replacing the previous
verified generated payload.
Cleanup uses only the validated file list and checks each listed file again
before deletion. It does not add new files to the deletion list.
Changed or unrelated files cause an error and remain unchanged. Do not store
application state in a generated package directory.

The installer checks the fixed path before the first write, including an empty
directory and its existing parents. It rejects reparse traversal. Updates check
the incoming manifest for collisions with unrelated files and directories before
removing any old file. Installation records the generated uninstaller hash in
the installed manifest. An untracked or changed uninstaller blocks an update.
Uninstall also checks every recorded file hash and path before removing services
or files. A changed file, missing uninstaller record or reparse path blocks it.
Helper launch failures and nonzero exits block the next step. A failed program
file or shortcut deletion reports incomplete removal instead of success.

Installer compilation requires NSIS 3.13. Set `MAKENSIS` to its executable if it
is not on PATH. A missing compiler is an error. Tool installation needs separate
approval. The output is `target\DomusObscuraSetup.exe`. Compilation does not
install the application or prove native service behavior.
Installer builds also include the compiler installation's exact `COPYING` file
under `notices\nsis-3.13` and record notice and compiler hashes in the component
index. This covers the NSIS runtime, standard plugins, graphics and compression
notices described by [NSIS](https://nsis.sourceforge.io/License). Keep the full
notice text, including the LZMA module terms, for distribution review #26.
Compiler warnings fail the installer build.

## Install

Review the package and notices before distributing it. Run the NSIS installer
only after approving installation on the selected machine. The application
section installs program files in the fixed `Program Files\Domus Obscura`
directory and creates a foreground shortcut. Keep writable state elsewhere.
Start this shortcut
as a normal user after the installer closes. The installer does not start an
elevated application.

The **Configure optional Windows services** section is off by default.
Selecting it asks for an absolute backend JSON path and an existing dedicated
local service account. It registers services with manual startup and leaves them stopped.
It does not create an account, grant service logon rights, change ACLs, initialize
media roots, configure certificates, alter a firewall, or start downloads.

Use one enabled non-administrator local account for both processes. Do not use
Administrator, Guest, LocalSystem, a personal administrator account, or two
different accounts. The current private-file policy trusts the runtime account
and Windows system administrators, not arbitrary shared-write groups.

Prepare and inspect these permissions separately:

| Resource                                                              | Runtime access                                          |
| --------------------------------------------------------------------- | ------------------------------------------------------- |
| Installed executables, browser files, scripts, and notices            | Read and execute; no write access                       |
| Local SQLite directory, database, journal/WAL files, and stable locks | Read and write for the dedicated account                |
| Backend JSON and its stable save lock                                 | Private access; write only when operator policy permits |
| Provider credentials, Entra references, TLS keys, and ACME state      | Private access for the same runtime account             |
| Already-bound staging and shared cache                                | Private access for the same runtime account             |
| Registered media roots                                                | Only the access required by approved operations         |

Keep writable state outside the install directory and outside all media roots.
SQLite must be on local disk. Use absolute host paths. Drive letters mapped in
an interactive session are not proof of service access. Check drive access under
the actual service account. Never redirect an unavailable drive to another root
or rebind a copied owner marker. Do not recursively grant permissions on an
existing library.

Grant **Log on as a service** through the machine's approved account procedure.
Keep passwords out of scripts, JSON, command arguments, logs, and answer files.
The helper uses `Get-Credential` and passes the protected credential to
`New-Service`. Account creation and password rotation remain manual.
Rotate the stored service credentials for both services through Windows service
administration before an account password expires.

Native startup checks the actual opened paths as the dedicated account.
Registration, an administrator's file read, and saved configuration validation
do not prove runtime credential or drive access.

## Start, stop, and status

Run the helper from the installed directory. Start, stop, install, and remove
need service-control permission; application runtime does not need administrator
membership.

```powershell
.\services.ps1 -Action Status
.\services.ps1 -Action Start
.\services.ps1 -Action Stop
```

The services are `DomusObscuraApi` and `DomusObscuraWorker`.
The Start action checks both service roles and their actual account SIDs before
starting either process. Both must use the same enabled non-administrator local
account. Stop and Remove remain available for an owned, misconfigured service.
Worker startup depends on API readiness. Only the API migrates SQLite.
An API service restart under a live worker service requires current schema and
cannot migrate it. Do not run a foreground worker against the same state while
using services. Stop every state user before a schema upgrade.

Running means initialization completed, not just process creation. Worker
readiness follows supervisor initialization. Stop is latched across API reloads.
The service control callback requests stop and returns promptly. Existing
application code drains operation threads, received/copy outputs, saved receipts,
certificate owners, and native locks before reporting stopped.

After both services pass the explicit Start action, the helper enables automatic
reboot startup and supervision. The Stop action disables automatic startup and
restart actions before draining. This prevents restart during a planned backup
or update. The next successful Start action enables them again. Supervision
allows two delayed process restarts per failure period, then stops retrying.
This is process recovery, not permission to approve work, reset transfer intent,
retry failed jobs, answer questions, or replace configuration.

Preshutdown uses a finite 180-second budget. Windows can still force termination
when its shutdown limits expire. Normal helper waits also have a finite limit.
A wait failure does not kill a process or prove it stopped. Inspect SCM status
before another operation.

The helper reports SCM state, account, PID, and exit codes. Application startup
failures produce a nonzero service-specific code: 10 for invalid inputs, 11 for
file/native I/O, 12 for SQLite, 13 for authentication, 14 for stale/conflicting
state, 15 for native ownership contention, 16 for remote/certificate failure,
and 99 for other failures. Codes contain no private input text. Inspect configuration and
actual account permissions after failure. Do not weaken secure configuration
to make a failed service start.

## Update and consistent backup

1. Stop worker, then API. Stop every foreground process using the installation.
2. Confirm stopped status. Prevent another operator or supervisor from starting
   either process during backup and update.
3. Back up the complete local SQLite state with all users stopped. Include
   any retained WAL/journal files. Alternatively use SQLite's supported online
   backup API under a separate documented procedure; do not copy a live database
   file alone.
4. Preserve configuration, root bindings, credential references, certificate
   state, staging, and recovery journals. Protect backup permissions.
5. Check the backup in a separate isolated copy. Do not open live media roots or
   run providers to test the backup.
6. Run the installer update. It rejects active services and foreground processes.
   Confirm the stopped-state backup before replacement.
7. Start API, check its current schema and active configuration, then start worker.
   Inspect durable recovery before approving any new operation.

The installer does not migrate SQLite or replace saved configuration.
After a schema upgrade, installing an older executable is not a database
rollback. Restore a tested, consistent backup only through an explicit recovery
procedure. Wider operating and restore procedures remain #19.

## Recovery and uninstall

Forced exit preserves committed transfer intent, sealed input receipts, native
root bindings, saved questions, and retained publication/recovery journals.
Ownership recovery waits for fenced lease expiry. Failed and waiting work still
requires explicit action. Incomplete scans remain distinct from confirmed
absence. Missing drives and denied credential files are errors, not empty state.

Stop both services before removal:

```powershell
.\services.ps1 -Action Stop
.\services.ps1 -Action Remove
```

Then run the uninstaller. It removes only exact installer-owned program paths.
It does not recursively remove the installation directory. Unknown files remain.
All configuration, databases, accounts, ACLs, credentials, cache/staging bindings,
media, and retained recovery data remain. A failed stop or service removal
blocks file removal. Partial setup stays available for inspection.

## Acceptance boundary

Foreground tests and package compilation are not native service or reboot proof.
Record actual restricted-account start/stop, active-operation drain, forced
exit/restart, missing drives, credential denial, update, uninstall, and reboot
results separately. Use a disposable Windows environment and fresh generated
state. Account/ACL changes, installation, and reboot require separate approval.
The deferred Azure Windows VM checks are tracked in #48. Run them after the
other outstanding work is complete. Native acceptance remains unverified until
those checks and distribution review are complete.

Official contracts:
[ServiceMain](https://learn.microsoft.com/en-us/windows/win32/services/service-servicemain-function),
[control and shutdown](https://learn.microsoft.com/en-us/windows/win32/services/service-control-handler-function),
[accounts](https://learn.microsoft.com/en-us/windows/win32/services/service-user-accounts),
and [NSIS](https://nsis.sourceforge.io/Docs/Chapter4.html).
