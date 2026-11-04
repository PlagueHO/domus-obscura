# Operator guide

## Scope and evidence

**The current baseline is local binary execution under an approved normal user
account.** This is a foundation application, not complete V1 or an approved public
deployment. Native Windows installer/services and Linux systemd deployment are
deferred in [#17](https://github.com/PlagueHO/domus-obscura/issues/17) and
[#18](https://github.com/PlagueHO/domus-obscura/issues/18).

Use this guide for foreground operation, maintenance, backup, and recovery.
The [Windows guide](windows-services.md) and [Linux guide](linux-services.md)
retain the optional package and service procedures. Their presence does not
prove installed-service or reboot acceptance. Windows host checks remain
[#48](https://github.com/PlagueHO/domus-obscura/issues/48).

| Evidence class                 | What it establishes                                                                                                                                                                                   | Limit                                                                                         |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Source-verified commands       | The CLI and guarded API implement the commands described here.                                                                                                                                        | Source inspection is not execution on the operator's host.                                    |
| Existing isolated checks       | Fresh databases, generated media, and local protocol/TLS fixtures exercise workflow and recovery rules. See [repeatable evidence](local-development.md#repeatable-evidence-without-live-credentials). | They do not prove live accounts, an existing library, installed services, or reboot behavior. |
| Operator recommendations       | The stopped-process backup and maintenance procedures below follow SQLite and application ownership rules.                                                                                            | No live backup/restore or host change was performed to write this guide.                      |
| Deferred privileged operations | Installer, account/permission, SCM, systemd, and reboot procedures remain available for review.                                                                                                       | They require separate approval and host evidence. Do not report them as passed.               |

Keep local results separate from CI results. Record the exact source revision or
package manifest for each result. Package hashes detect changes; they do not
authenticate the publisher. Distribution review remains
[#26](https://github.com/PlagueHO/domus-obscura/issues/26).

## Prepare and install for foreground use

Choose one normal, non-administrator runtime account. On Linux, do not run the
application as root. Use the same account for the API and worker. Accounts used
by optional services need separately approved setup; do not grant administrator
membership to resolve an access error.

For a source checkout, install the approved build prerequisites first. Use the
repository tool pins. Bootstrap restores dependencies; it does not install host
tools.

```powershell
.\dev.ps1 bootstrap
.\dev.ps1 dev
```

On Linux:

```sh
cargo task bootstrap
cargo task dev
```

The development task builds the real browser and starts the foreground API.
The separate sample browser harness is not an operating mode for an existing
library.

For a release candidate, obtain a trusted payload built for the selected OS.
Follow the platform guide's package checks. Put a verified payload in a new,
approved program directory. Keep the executable and sibling `browser` directory
together. Windows payloads also include their native dependency notices and
helpers. Linux candidates target Ubuntu 24.04 x86_64 GNU/Linux; another platform
needs separate verification.

Keep writable state outside the program directory, Git, and media roots.
Never put application state in a generated package output directory.

| Resource                                                | Required access and location                                                                                     |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Executable, browser assets, helpers, notices            | Runtime read/execute. Optional service payloads must not be runtime-writable.                                    |
| SQLite directory and sidecars                           | Private runtime read/write on local disk. Never use an SMB share or other network filesystem.                    |
| Backend JSON and stable save lock                       | Private access for the runtime account. Editable configuration also needs sibling-file creation and replacement. |
| Credential references and TLS private keys              | Private files accessible to that account. Keep contents out of commands, logs, URLs, examples, and Git.          |
| Staging, managed certificate state, selected file cache | Approved private directories with the required lifecycle write access and native bindings.                       |
| Registered main/backup media roots                      | Read access for scans. Grant write access only for separately approved media operations.                         |

Check access under the actual runtime account. An administrator's successful
read is not runtime access evidence. Windows mapped drives from an interactive
session do not prove service access. Linux service namespaces can hide paths
that a foreground account can read. Do not recursively change permissions on
an existing library.

## Start and check status

### Default single-user launch

From the verified program directory on Windows:

```powershell
.\domus.exe version
.\domus.exe serve
```

On Linux:

```sh
./domus version
./domus serve
```

On a fresh first launch, open `http://127.0.0.1:8080`. With no saved configuration,
the command creates explicit `single_user` anonymous HTTP on loopback with the
fixed `_anonymous` identity. It needs no certificate, Azure, DNS, or Entra setup.
Provider services are disabled and system settings are read-only by default.

First launch creates private `DomusObscura` state below `LOCALAPPDATA` on Windows,
or `XDG_DATA_HOME` on Linux. Linux falls back to `$HOME/.local/share`.
Later launches load the existing `service.json` and reuse `state.db`. Omitting
the configuration argument does not reset saved authentication, provider, or
certificate settings. Use the saved origin and review configured startup effects.
Inspect retained state after a failed first launch. Existing configuration and
pending files are not replaced automatically.

Only direct loopback peers are accepted. Host, exact Origin, `x-domus-csrf`,
parser, and connection guards remain active. Do not put this anonymous endpoint
behind a public proxy.

### Configured API and separate worker

Use an absolute path to the approved private backend JSON. The placeholders
below are not literal paths. Replace them locally; do not publish private paths.

```powershell
$Configuration = '<absolute-private-configuration-file>'
.\domus.exe serve $Configuration
```

In a second terminal under the same account, after API readiness:

```powershell
$Configuration = '<absolute-private-configuration-file>'
.\domus.exe worker $Configuration
```

On Linux, use the same path in both terminals:

```sh
configuration='<absolute-private-configuration-file>'
./domus serve "$configuration"
```

```sh
configuration='<absolute-private-configuration-file>'
./domus worker "$configuration"
```

Use this startup order:

1. Confirm that no other deployment or process owns this state or live peer
   account. Review pending work before enabling a worker.
2. Prepare initial state, private references, and native roots separately.
   With explicit configuration, the local database must already exist.
   `domus init <local-database>` creates or migrates SQLite; run it only as the
   designated setup owner with every state user stopped.
   Follow [native root binding](../README.md#native-root-binding) before using
   existing media or staging. Registration, binding, and media approval are
   separate operations.
3. Start one API with the intended executable and configuration. API startup can
   migrate SQLite and initialize configured host resources; it is not validation.
4. Confirm that the browser responds at the configured origin. Inspect the active
   API revision and startup errors. Do not use process creation alone as readiness.
5. Start one worker only if `production_worker` is configured and its work is
   intended. Do not also enable `local_acquisition`. The worker requires the
   current schema and never migrates it.
6. Check the fresh worker status and its configuration revision. Review retained
   questions, requests, and recovery records before submitting new work.

Starting a process grants no new plan, source, or JPEG approval. A worker can,
however, resume interrupted accepted work from durable evidence after ownership
checks. A managed API can resume certificate lifecycle work. Startup is therefore
not an effect-free way to test a backup.

Production configuration requires the permitted Entra tenant/object pair and
HTTPS. Explicit development uses anonymous loopback HTTPS, not the default HTTP
mode. Follow [developer HTTPS](local-development.md) or
[production authentication](../README.md#production-authentication).
A failed explicit secure configuration never falls back to anonymous HTTP.
The API preview remains loopback-only; a production configuration does not
authorize public exposure.

The optional Windows service helper starts both roles in one Start action. It
does not pause for API-only inspection. Use the Windows guide's separate
maintenance procedure when a schema upgrade or recovery requires that boundary.

### Status sources

There is no general foreground `domus status` command.

| Question                                     | Check                                                                                                                                                                       |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Is the process still running?                | Inspect its terminal and exact PID. On Windows, use `Get-Process -Id <pid>`; on Linux, use `ps -p <pid>`. PID existence alone does not prove readiness or drain completion. |
| Which API configuration is active?           | Open System, or read guarded `GET /api/system/configuration`. Compare `active_revision`, `saved_revision`, validation state, and reload state.                              |
| Is the worker using the saved configuration? | Read the browser's fresh worker status and configuration revision. A missing/stale heartbeat or failed refresh is not current readiness evidence.                           |
| Was a request completed?                     | Read `GET /api/worker/requests/{id}`, the job history, and transfer/publication receipts. HTTP 202 means admitted, not completed.                                           |
| Why is work waiting?                         | Inspect Decisions, provider status, and Recovery. Saving an answer, applying recovery, and retrying are separate actions.                                                   |
| Which certificate is active?                 | Use the authenticated certificate status view for the active API revision. Local `certificate-status` checks committed state, not live listener activation.                 |

Do not use `worker-check` as a general production worker status command. It belongs
to the separate one-shot worker workflow. See the
[worker guide](../README.md#separate-production-worker) for bounded queues,
request filters, and explicit retry/dismiss commands.

## Stop and exclude all state users

This is the maintenance boundary for backup, restore, and upgrade.

1. Assign one maintenance owner. Stop new submissions and coordinate with every
   other operator. Prevent automatic or manual restarts for the whole procedure.
2. Stop the worker first with Ctrl+C in its foreground terminal. On Linux,
   SIGTERM uses the same drain path. Wait for operation/session threads to finish
   and for the process to exit.
3. Stop the API with Ctrl+C. Linux SIGTERM also requests normal drain. The local
   API closes HTTP connections with a ten-second grace period, then waits for
   active local work and result recording. That operation drain has no forced
   timeout.
4. Stop every other state user: one-shot workers, source tools, scan commands,
   `service-call`, `mcp-stdio`, standalone certificate bootstrap/recovery,
   SQLite tools, and external configuration writers. Confirm their actual exit.
5. If optional services exist, use their platform stop procedure. Windows helper
   Stop disables automatic startup/restarts. Linux maintenance disables boot
   startup and verifies that its service activation block is effective before
   copying state. Runtime masking can fail or be ineffective for local unit files;
   follow the Linux guide's checks. Neither service procedure prevents a separate
   foreground launch; coordinate those users too.
6. Confirm stopped state before copying or replacing anything. A wait timeout,
   stale heartbeat, lost response, or closed browser does not establish it.
   If a process remains active, stop maintenance and inspect the condition.

Do not kill a process to meet a backup deadline. Forced exit is a recovery
condition, not successful drain. Preserve committed intent, receipts, incomplete
scan coverage, and journals. Later recovery must wait for fenced ownership.
Never delete a stable `.domus-*.lock`, configuration lock, staging `owner.lock`,
or certificate owner marker to bypass contention.

Service managers have finite shutdown budgets and can force termination.
Those limits do not apply to every foreground operation. See the platform guides
before relying on SCM or systemd state.

## Validate, save, and activate configuration

Backend JSON and versioned Music output policy are different state. The JSON has
a raw-byte revision; immutable output settings and plans remain in SQLite.
Changing one does not rewrite existing plans or approve media work.

| State                      | Meaning                                                                                               |
| -------------------------- | ----------------------------------------------------------------------------------------------------- |
| Active                     | The API's startup snapshot, or the worker revision confirmed by a fresh heartbeat.                    |
| Saved, unvalidated         | JSON on disk that has not passed validation for its exact revision. Inspection does not validate it.  |
| Saved, passively validated | A successful service save checked that revision. Referenced files can still change before activation. |
| Reload/restart required    | Saved JSON differs from the API snapshot. API activation and worker activation are separate.          |

Use System to inspect settings. When both active and saved configuration permit
editing, use **Validate section** before **Save section**, or save related changed
sections together. Validation reads existing inputs, verifies TLS, and queries
current-schema SQLite. It does not migrate, create/bind roots, contact providers,
start work, or activate TLS. There is no general backend-validation CLI.
Do not substitute `serve`, `api-preview`, or `PreparedServer::load` for validation.

The backend enforces `system_settings_read_only` on each protected write command. The browser
cannot change that setting. Keep hidden references unchanged unless a write-only
replacement is intended. A blank replacement must not erase a saved secret.

After a failed save, retain the draft. Refresh and review the saved revision
before retrying. A pre-replacement failure preserves the previous file. A sync or
verification failure after replacement can leave an uncertain saved result; do
not claim rollback. Inspect an interrupted owned `.pending` file with the API
stopped. Never delete configuration or its stable lock as automatic recovery.

**Reload** requests explicit, revision-bound API activation. Preparation failure
keeps the old host active. A failed listener restoration is fatal and needs
operator recovery. After an origin or identity change, reconnect at the intended
origin, sign in if required, and check the active revision. A pending command or
lost response does not prove activation. Do not retry reload automatically.

API reload does not restart the worker. Stop the worker at a safe boundary, then
start it with the reviewed saved configuration. Confirm its fresh revision.
Accepted requests retain their configuration digest; a changed provider/account
or staging choice cannot silently replace it.

For manual edits, stop the API first. Stop the worker too when its configuration
changes. Preserve private permissions and the stable sibling lock. Restart in
API-then-worker order. Tools that edit live JSON must implement the full
[configuration writer protocol](agent-operations.md#backend-configuration-writers);
atomic rename alone is not sufficient.

## Certificates and peer networking

Default single-user HTTP needs no certificate. Explicit HTTPS needs a valid
hostname/date, certificate chain, and matching private key. Never disable TLS
verification or install machine trust to hide an invalid configuration.

Manual TLS and Azure DNS ACME are separate choices. Managed state includes private
keys, generations, account/order journals, DNS intents, and cleanup receipts.
Keep its native root and private permissions. Stop the API before standalone
issuance or recovery. `certificate-status` is passive; `certificate-bootstrap`
and some recovery actions contact ACME/Azure and can change DNS. Get separate
approval for those effects. Use the
[certificate guide](../README.md#optional-azure-dns-certificates) for exact actions.
A matching TXT value alone does not prove cleanup ownership. Do not redirect
pending cleanup to a different DNS tenant/resource after credential rotation.

Provider selection does not authorize live traffic. Acquisition needs an approved
source, separate private account credentials, explicit plaintext-login
acknowledgement, and the normal plan/JPEG approvals. Do not use the same live
Soulseek account in another deployment. Restart does not reset uncertain intent.

Sharing defaults to **None**. An explicit sharing listener selects an IPv4 address
and nonzero port. No automatic port mapping is enabled. Without a listener,
outgoing peer connections cannot provide reverse file acceptance. Firewall,
router, public-port, and live-account changes need separate approval.
Keep the API's loopback rules separate from peer listening.
Soulseek usernames are not cryptographic identity. See
[read-only sharing](../README.md#read-only-soulseek-sharing) for access policy,
native folder binding, queue limits, and retained spool handling.

MCP is local stdio only. Never add a public MCP endpoint. A local MCP or provider
process that opens this deployment's state is still a maintenance state user.

## Consistent backup

### Inventory and recovery objective

**An application-state backup is not a complete media backup.** SQLite records
plans, approvals, bindings, and receipts; it does not contain every audio file,
retained album, credential file, or remote DNS/provider state.

Choose the recovery objective first. Use one private record for the executable
version/package manifest, source revision when available, snapshot identifier,
saved configuration revision, state locations, expected native roots, file
inventory/hashes, and included media snapshot. Record unavailable items as errors,
not empty directories. Keep the record and backup outside Git.

| Backup item                    | Preserve                                                                                                                                                                |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SQLite                         | The configured database plus retained matching `-wal`, `-journal`, and any other SQLite sidecars. Keep the entire stopped set together.                                 |
| Backend configuration          | Exact JSON bytes, reference mapping, private permissions, and pending-save evidence.                                                                                    |
| Runtime ownership              | Native root/ancestry records in SQLite and owner/lock evidence in its original location. Copies of markers do not bind a replacement root.                              |
| Staging and immutable evidence | Sealed inputs, normalized files, approved JPEGs, recipes, transfer/normalization/publication receipts, and pending journals.                                            |
| Retained media                 | Original and restore-displaced album versions, unfinished destination copies, and corresponding recovery records. These can be outside application state.               |
| Credentials and TLS            | Separate credential files, private keys, manual certificates, managed generations/journals, and DNS write/cleanup evidence. Protect the backup as private keys.         |
| Selected cache                 | Root ownership and configuration. Evictable cache entries are distinct from immutable approvals and receipts; missing cache data is not permission for SQLite fallback. |
| Media snapshot                 | Required main/backup files and retained versions when the recovery objective includes media. An indexed observation is not a media backup.                              |

Stable lock files are not disposable cache. Keep originals in place. Record their
evidence, but never restore a copied lock over an original to bypass ownership.

### Take a stopped-process backup

1. Select a new private backup destination with sufficient space. Keep the source
   roots and SQLite on their approved storage. Check backup access permissions.
2. Complete [stop and exclusion](#stop-and-exclude-all-state-users). Do not restart
   a process until copying and verification finish.
3. Freeze external media writers or obtain a separately coordinated media snapshot
   if matching media is part of the objective. Application locks do not exclude
   arbitrary external programs.
4. Copy the full stopped SQLite set. After a crash, committed changes can remain
   in the WAL, or rollback recovery can need a journal. Never copy a live database
   alone, delete a sidecar, or force a checkpoint as a backup shortcut.
5. Copy each required inventory item without changing the source. Preserve private
   access, file contents, and pending evidence. If a root is unavailable or a file
   changes during copying, report an incomplete backup and do not use it for restore.
6. Record the completed file inventory, sizes, and hashes. Compare source and
   backup while they remain stopped. Confirm that every required item is included.
7. Verify a separate working copy as described below. Retain the untouched backup.
   Resume the original installation in API-then-worker order only after review.

The `-shm` file coordinates WAL readers; it is not the committed transaction log.
Do not infer from its absence that the WAL is unnecessary. Preserve retained
sidecars in the stopped set. Let a compatible SQLite library manage coordination
and recovery on the disposable verification copy, not on the only backup.

### Verify without starting the application

Copy the backup's database and matching sidecars into a new private working
directory on local disk. The copy must be writable if SQLite needs crash recovery
or shared-memory coordination. Do not open the live database, the untouched backup,
or a copy attached to live media roots.

With an approved SQLite tool, open only that working database and run:

```sql
PRAGMA integrity_check;
PRAGMA foreign_key_check;
PRAGMA user_version;
```

`integrity_check` must return `ok`. `foreign_key_check` must return no rows.
Record `user_version` against the selected executable's migrations. A newer
schema is not compatible with an older executable. These checks do not migrate
Domus tables. If the tool is missing, report verification as not run; tool
installation needs approval.

Close the tool. Keep its results separate from hashes of the untouched backup;
SQLite recovery can change the working copy. Do not launch the API/worker, bind
roots, or call providers to test a real backup. Integrity success establishes
database structure, not matching media, native identities, or successful
application restore.

SQLite's [online backup API](https://www.sqlite.org/backup.html) is a supported
database snapshot option, not a Domus backup command. It does not capture matching
staging, certificate, configuration, media, or external state. A separate reviewed
coordination procedure is required before using it for an application recovery
objective. See SQLite's
[backup corruption risks](https://www.sqlite.org/howtocorrupt.html#backup_or_restore_while_a_transaction_is_active)
and [WAL requirements](https://www.sqlite.org/wal.html). The official
[integrity-check](https://www.sqlite.org/pragma.html#pragma_integrity_check) and
[foreign-key-check](https://www.sqlite.org/pragma.html#pragma_foreign_key_check)
contracts define the verification results above.

## Restore a backup

**Routine restore is same-host recovery with compatible state and original native
roots.** A path with the same text or a copied owner marker is not the same root.
File identity, ancestry, contents, and receipts still need their normal checks.
This guide does not authorize transplanting a bound deployment to a new host/root.

1. Choose and verify the complete backup set. Identify a compatible executable.
   Decide whether the operation is recovery or an intentional schema upgrade.
2. Complete [stop and exclusion](#stop-and-exclude-all-state-users). Preserve the
   current stopped database/sidecars, configuration, files, and recovery evidence
   in a separate private set before any replacement.
3. Compare all effects since the snapshot. New media publications, retained-album
   restore/deletion, uncertain peer submissions, DNS writes, credential rotation,
   and session revocation can make an old snapshot unsafe. Do not discard newer
   receipts or assume rollback undoes remote actions.
4. If any such effect exists or is unknown, stop routine restore. Reconcile it
   through a separately reviewed recovery procedure using both evidence sets.
   Restoring old SQLite can revive accepted work, old sessions, or stale DNS
   intent. A backup cannot supply proof that a submission did not occur.
5. Confirm that original media, staging, cache, and certificate roots still have
   the expected native identities and access. Do not delete/recreate those roots
   or rerun initialization over them. A changed identity requires explicit supported
   recovery, not marker copying or a new empty folder.
6. Restore only the selected database set and matching configuration/evidence
   after the preceding conditions pass. Preserve the original parent/root and
   stable locks. Do not overlay active state, mix old and new sidecars, or leave
   the replaced database's WAL beside the restored database. Preserve displaced
   files in the pre-restore set rather than delete unknown evidence.
7. Restore required external files only under the reviewed media/credential
   recovery procedure. Recheck original bindings, exact bytes, and private
   permissions. Matching hashes alone do not prove native identity. Partial or
   different files remain errors, not permission to overwrite or adopt them.
8. Start only the selected API after all pre-start checks pass. This is real
   activation: migration, managed TLS, and configured host resources can have
   effects. Inspect startup errors, active configuration, Decisions, requests,
   and Recovery before starting a worker.
9. Start the worker only after reviewing resumed work and account ownership.
   Confirm its fresh revision and durable results. Do not automatically retry
   failed work or approve changed observations. Retain both backup sets until
   recovery is confirmed.

If the only available backup has missing native roots or incomplete external
evidence, report restore as blocked. Fresh state/bootstrap is a separate recovery
decision; it does not preserve or erase unresolved old work.

## Update with one migration owner

1. Obtain and verify the new executable, browser assets, and notices separately.
   Keep a trusted copy of the previous payload and its manifest.
2. Assign one migration owner. Complete stop/exclusion and the verified backup
   procedure before replacing program files.
3. Confirm no worker, API, CLI, certificate owner, or SQLite tool is active.
   `init` and some standalone CLI opens can migrate; do not run them concurrently
   or assume service startup guards protect every foreground command.
4. Replace only verified program files using the platform procedure. Keep JSON,
   state, private references, bindings, and media separate. Changed or unknown
   installed files block removal; inspect the error instead of forcing deletion.
5. Start one new API as the upgrade migration owner. Do not start a second
   migration-capable process. The worker must remain stopped until the schema
   and active configuration are confirmed.
6. Inspect retained questions, publication/transfer state, and certificate recovery.
   Start the worker with the reviewed saved configuration. Confirm its revision.
7. Restore any approved supervision only after readiness and recovery checks.
   Native service enable/start and reboot acceptance remain deferred.

Installing an older executable after migration is not a database rollback.
Use only the restore procedure and a compatible, verified backup. Do not reset
`user_version`, edit migrations, or discard receipts to make an older binary run.

## Faults and explicit recovery

| Condition                                             | Safe action                                                                                                                             | Do not                                                                                                         |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Missing drive, denied access, or changed root binding | Restore access to the original root. Review recorded errors and incomplete scan coverage before explicit retry.                         | Create a replacement empty root, copy a marker, or treat unreadable coverage as confirmed absence.             |
| Provider timed backoff                                | Wait for its saved interval. Restart retains the delay.                                                                                 | Clear locks/cache or restart repeatedly to shorten it.                                                         |
| Unsupported provider pause                            | Inspect provider status and reason. Use the matching explicit resume action after review.                                               | Clear an ordinary timed delay, adopt a live owner, or assume resume sends a request.                           |
| Failed/waiting worker request                         | Save an offered answer if needed, apply its separate recovery action, then explicitly retry the saved request.                          | Treat HTTP 202 or a saved answer as completion; automatically retry on restart.                                |
| Forced exit or stale ownership                        | Inspect committed intent and receipts. Wait for fenced lease expiry before supported continuation.                                      | Delete stable locks, reuse a worker name as authority, or invent a fresh request to bypass old ownership.      |
| Uncertain transfer submission                         | Preserve intent and independent completion/receipt evidence. Use only its supported reconciliation path.                                | Reset submission, resend `QueueUpload`, or infer no effect from a lost response.                               |
| Sealed received inputs                                | Use retained exact receipts for supported local continuation.                                                                           | Reconnect to the peer or rewrite input just to repeat a completed receive.                                     |
| Partial/different publication output                  | Preserve it and inspect the saved journal/error. Exact crash-gap verification can complete only matching work.                          | Overwrite, adopt, or recreate missing receipted files.                                                         |
| `backup_pending`                                      | Keep verified primary and retry input. Restore the original backup root, then explicitly continue the same publication.                 | Treat primary success as completion of all destinations or rerun acquisition.                                  |
| Retained album restore/deletion                       | Review the exact retained version and its current evidence. Confirm each operation separately.                                          | Infer permission from acquisition/cover approval or delete the visible album through retained-version cleanup. |
| Cleanup failure after publication                     | Inspect Recovery and its saved cleanup error. Explicitly retry the exact cleanup. Album completion and cleanup completion are separate. | Erase receipts, remove pending-backup inputs, or redownload an album to clean scratch.                         |
| Changed configuration or cache failure                | Refresh and review saved/active revisions and the selected native cache. Preserve drafts and immutable evidence.                        | Show stale values as current, bind a replacement root, or fall back to SQLite.                                 |
| Certificate uncertainty or cleanup failure            | Preserve account/order/DNS journals and generations. Follow exact local recovery with separately approved live effects.                 | Remove a TXT value without ownership evidence, create replacement orders, or disable certificate checks.       |

Library browsing is read-only. A completed media operation does not automatically
provide fresh index observations. Use the intended explicit scan/refresh after
work finishes. See [local recovery](local-development.md#failure-and-recovery)
and [Recovery inventory](../README.md#recovery-inventory-and-deletion) for the
installed browser actions and their limits.

## Uninstall without deleting user state

1. Complete stop/exclusion. Preserve a verified backup if the installation must
   remain recoverable.
2. For a manually placed payload, inspect its verified file inventory. Remove
   only the exact unchanged program files under separate deletion approval.
   Do not recursively delete an unknown directory or an application-state root.
3. If optional services/installer were used, follow the platform removal procedure
   only after separate approval. A failed stop/removal blocks program deletion.
   Preserve changed unit files and operator-authored drop-ins.
4. Leave accounts, permissions, configuration, credentials, SQLite, cache/staging
   roots, certificates, media, and retained recovery unchanged. Report partial
   removal explicitly.

Uninstall is not recovery-data disposal. Deleting those retained items needs a
separate reviewed operation with exact ownership and evidence.
