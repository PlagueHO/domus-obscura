# Process-crash recovery evidence

The optional [disposable-volume lab](crash-recovery-lab.md) adds explicitly
selected filesystem-full and detached-volume probes. Its scripts require
separate execution approval and verified disposable roots. Recorded runs below
distinguish passing cases from failed or unperformed cases.

## Boundary and limits

The generated process tests exercise the real Rust application operations in
separate test-host processes. The parent waits for a named checkpoint, checks
committed state, terminates that exact child, observes abnormal exit, and starts
a new process against the same SQLite database and native roots.

The checkpoint helper and its calls use `cfg(test)`. Normal application builds
have no checkpoint helper, control channel, environment switch, or crash command.
Shared fixtures use generated MP3, FLAC, images, approvals, and fresh local state.
The tests do not use live accounts or existing library folders.

Recovery waits for the actual saved claim lease to expire. It does not change
lease timestamps or replace the production lease duration. The matrix starts
independent recovery children together so those waits overlap. Each child has
a bounded deadline. Parent guards terminate and reap unfinished owned children.
The ignored `crash_child` test is a private harness entry point, not an acceptance
case to run manually. Its owning tests select it explicitly.

This is process-crash evidence, not OS-crash or power-loss evidence. It does not
prove that a storage controller preserves writes through power loss. It does not
prove installed-service supervision, restricted-account setup, reboot startup,
physical drive removal, or a full host volume. Native deployment checks remain
deferred under [#17](https://github.com/PlagueHO/domus-obscura/issues/17) and
[#18](https://github.com/PlagueHO/domus-obscura/issues/18).

## Run the checks

Use the pinned toolchain and native build environment:

```powershell
.\dev.ps1 test --lib crash_tests
.\dev.ps1 check
```

On Linux:

```sh
cargo task test --lib crash_tests
cargo task check
```

The existing Windows and Ubuntu 24.04 CI `check` jobs discover these tests
through the normal Rust workspace suite. A local Windows pass is not a Linux CI
result. Record the tested commit, platform, command, failures, and skipped checks
with issue [#22](https://github.com/PlagueHO/domus-obscura/issues/22).

## Forced-stop matrix

`cross_process_crash_recovery_matrix` covers 37 checkpoint/format cases:

| Operation                   | Checkpoints                                                                                                                                                      | Required outcome                                                                                                                                                         |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Transfer intent             | Committed source and complete file intent, before submission                                                                                                     | Keep the exact source and three planned transfers. Reject another source and stale claims.                                                                               |
| Uncertain submission        | Queue request sent, before its reply is persisted                                                                                                                | The loopback peer observes one submission. Recovery sends a queue query, not another submission. A lost query response keeps uncertainty.                                |
| Receive                     | First partial write; synced stream without a sealed receipt; sealed input before SQLite receipt; SQLite receipt committed                                        | Preserve partial or unsealed bytes with an error. Reconcile only exact sealed input. Reject another start for a received transfer.                                       |
| Normalization, MP3 and FLAC | Run intent; output write; synced output before receipt; first audio receipt; before publication handoff                                                          | Reclaim uses a new generation-bound run. Keep earlier run files and receipts. Check the new run's complete receipts without reusing stale authority.                     |
| New publication             | Destination intent; partial audio copy; synced file before receipt; file receipt; verified folder before rename; final rename before journal update              | Keep partial output unchanged with an error. Reconcile exact completed effects. Do not normalize or download again. Verify final hashes.                                 |
| Replacement                 | Original folder retained before journal update; replacement visible before journal update                                                                        | Preserve original and replacement file identities and hashes. Continue the same approved operation.                                                                      |
| Restore                     | Intent; current folder rename; current-moved journal update; original rename; original-visible journal update; displaced-current rename; restored journal update | Preserve both versions. Continue using the same immutable approval. Repeated completion performs no new renames.                                                         |
| Retained deletion           | Intent; first file unlink before receipt; empty retained directory removal before receipt; completion saved                                                      | Keep current content and all files not approved for deletion. Reconcile absence only after removal intent. Block restore and repeat completion without another deletion. |
| Durable questions           | Question committed; answer committed before explicit recovery                                                                                                    | Preserve one version-bound question and its choices. An answer alone leaves the job waiting.                                                                             |

The parent checks SQLite integrity and foreign keys. It compares retained file
identities, lengths, and hashes across restart. It also checks directory
identities across publication and restore moves. Unrelated sentinel content must
remain unchanged.

Restore continuation refreshes the state-bound command preview after each
journal step. It does not create another approval. The test compares the saved
approval hash, album identity, and file list before and after each continuation.

`cross_process_recovery_rejects_changed_approved_content` adds two forced-stop
cases. After interruption, it changes one generated restore-displaced file or
one remaining deletion file. A new process must reject continuation with the
saved approval and preserve the changed bytes. A new approval must not be
inferred from a retry.

## Controlled faults

| Fault                    | Evidence                                                                                                                                                                                                                        | Limit                                                                                                |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Unavailable backup       | Rename only the generated backup root while no operation owns it. A child records `backup_pending`. Return that same root and continue in another process with a new returned claim. Keep primary content and retry input.      | Simulated path unavailability, not physical drive removal or unmount.                                |
| Disk-full write          | Inject the native storage-full error after a real partial publication write. A new process rejects repair and leaves that output unchanged.                                                                                     | Injected filesystem failure, not a full host volume.                                                 |
| SQLite full              | Set a fresh store connection's page limit. Run real admission transactions until SQLite reports `DiskFull`. Check rollback and database integrity.                                                                              | Real SQLite failure on limited pages, not filesystem exhaustion. This case is in-process.            |
| SQLite contention        | Another child holds a real write transaction. Check the bounded busy error and unchanged event state. Terminate the holder and retry explicitly.                                                                                | Local SQLite only.                                                                                   |
| Native staging ownership | Another child holds the real staging lock. Reject a competing open, then reacquire after death without changing the owner marker identity.                                                                                      | Does not prove service-manager behavior.                                                             |
| Windows file sharing     | Another child opens a sealed input with sharing denied. Reject reading while held, then verify the same input after death.                                                                                                      | Windows-specific. Linux advisory locks do not exclude arbitrary external writers.                    |
| Provider response loss   | The loopback protocol fixture counts queue requests and recovery queries while responses are lost across process death.                                                                                                         | Generated local peer, not a live account or external provider.                                       |
| TLS rejection            | Existing `oidc_transport_verifies_private_ca_bounds_bodies_and_never_follows_redirects` and TLS material tests use local certificates and listeners. They reject untrusted certificates, wrong hostnames, and invalid material. | Separate transport evidence, not process termination during TLS activation or a live provider check. |

## Recorded local results

Local validation on 2026-11-06 used Windows x86_64 MSVC and the pinned Rust
toolchain. The working tree was based on
`ea8c6b241997bc2c79eaa52f517f673a402142b0`, with the uncommitted changes for
issue #22. These results are not CI evidence for a pushed commit.

| Check                    | Local result                                                                                                                                         |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Process-crash matrix     | Passed all 37 checkpoint/format cases, including real lease expiry and fresh-process continuation.                                                   |
| Changed approved content | Passed both restore and retained-deletion rejection cases.                                                                                           |
| Controlled faults        | Passed the unavailable-backup, injected disk-full, SQLite-full, SQLite-contention, staging-lock, and Windows sharing-denial cases.                   |
| Media regression suite   | Passed all 103 ordinary media tests. The separate installed-service entry point remained ignored.                                                    |
| HTTPS regression suite   | Passed all 37 tests, including TLS trust, hostname, material, and response-loss checks.                                                              |
| Full repository check    | `.\dev.ps1 check` passed contracts, rustfmt, Clippy, workspace tests, generated harness tests, Prettier, frontend types, and all 266 frontend tests. |

The ignored crash child is a private entry point used by the passing parent
tests. It is not a missing crash case. The scale tests remain separate and were
not run because this change does not change indexing or query behavior.

## Recorded CI results

Evidence review on 2026-11-06 confirmed that
[PR #56](https://github.com/PlagueHO/domus-obscura/pull/56) merged the harness.
Its successful [PR workflow](https://github.com/PlagueHO/domus-obscura/actions/runs/42172196630)
reports head SHA `ea5ad0620c0c7a0f9e37ae270052247c1be720b1`.
Both checkout logs identify the tested PR merge revision as
`b34cf22bcf4415041578f236489c282a51786c30`, which merges that head into
`ea8c6b241997bc2c79eaa52f517f673a402142b0`.
This is not the final merge commit on `main`.

| Platform     | Evidence                                                                                               | Verified result                                                                                                                                                                                |
| ------------ | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ubuntu 24.04 | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42172196630/job/127742491151) | `cargo task check`, build, and smoke passed. All six applicable crash/fault parent tests passed, including the 37-case matrix and both changed-content rejection cases.                        |
| Windows      | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42172196630/job/127742491218) | `cargo task check`, build, and smoke passed. All seven crash/fault parent tests passed, including the same matrix and rejection cases, plus the Windows-only sealed-input sharing-denial test. |

Both `cargo task check` logs report 354 passed, zero failed, and two ignored
library tests. They explicitly report a passing crash matrix, rejection of
changed approved content, unavailable-backup recovery, simulated disk-full handling,
SQLite-full atomicity, and database/staging ownership tests. The private
`crash_child` entry point is ignored in ordinary discovery and selected by its
parents. Its ignored status is not a missing acceptance case. The separate
HTTPS suites passed all 37 tests on each platform. These are transport results,
not forced termination during TLS activation.

### Verified final merge results

The [final merge workflow](https://github.com/PlagueHO/domus-obscura/actions/runs/42175378096)
completed successfully for `40da8c00cc34bfe0d70789bd15eff2f697575cb0`.
Both platform checkout logs identify that exact final merge revision. These
completed results replace the earlier in-progress status for this workflow.
The historical local and PR results remain separate.

| Platform     | Evidence                                                                                               | Verified result                                                                                                                                                                   |
| ------------ | ------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ubuntu 24.04 | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42175378096/job/127752512263) | Check, build, and smoke passed. All six applicable crash/fault parent tests passed, including the 37-case matrix and both changed-content rejection cases.                        |
| Windows      | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42175378096/job/127752512327) | Check, build, and smoke passed. All seven crash/fault parent tests passed, including the same matrix and rejection cases, plus the Windows-only sealed-input sharing-denial test. |

Each final merge library suite reports 354 passed, zero failed, and two ignored.
The private ignored `crash_child` entry point is used by the passing parents.
Both logs also confirm unavailable-backup recovery, simulated disk-full handling,
SQLite-full atomicity, and database/staging ownership checks. Each separate HTTPS
suite passed all 37 tests. Both pagination jobs passed, but pagination does not
prove crash recovery.

These are remote results for the stated final merge revision, not new local
test results or proof for later commits. The HTTPS results remain separate
transport evidence, not process termination during TLS activation.

## Follow-up TLS and volume lab

The follow-up working tree is based on
`44a4c67a6bc59d0577d99d42947f6ffbfdbacd1e`. Its local results are not CI
evidence or a result for a committed follow-up revision.

`backend_configuration_tls_crash_manual_reload_boundaries` terminates owned
server children at `preparing`, `prepared`, `handover`, `before_report`, and
`reported`. Fresh processes use the same saved configuration. Real TLS
handshakes check the served certificate. Saved and active revisions remain
separate evidence.

`backend_configuration_tls_crash_invalid_manual_keeps_live_host_but_rejects_restart`
checks rejected replacement material and rejects startup from invalid saved
configuration without anonymous fallback.
`backend_configuration_tls_crash_managed_rejection_preserves_journal_and_generations`
checks staging and untrusted production candidates before and after restart.
It preserves journal and immutable generation evidence. These three parent
tests passed locally on Windows. The ignored TLS child is their private
entry point.

Successful managed production activation remains unverified. Generated
certificates cannot replace the independent public trust requirement. Manual
TLS reload results do not prove managed issuance, ACME or DNS effects.

The opt-in volume lab adds six cases: publication full, receive full,
normalization full, database filesystem full, detached backup, and wrong
backup volume. They are ignored in ordinary discovery and require explicit
selection through the lab wrappers. Adding these cases does not prove they
passed. The wrappers' no-host-configuration-change checks and the Rust
ownership rejection tests are distinct from volume acceptance.

Local Windows validation passed `.\dev.ps1 check`, including the existing
37-case matrix, all three new TLS parents, 359 library tests, the separate
media and HTTPS suites, and 266 frontend tests. The library suite reported
10 ignored entry points and opt-in cases. The final lab-only path and filler
guard refinements then passed `.\dev.ps1 test --lib volume_lab_` with three
passed and seven explicitly ignored lab entries. Workspace all-target Clippy
with denied warnings passed after those refinements. Both wrapper safety
suites and `pnpm format:check` passed. These local runs do not establish
follow-up release-build, smoke, scale, or remote CI results.

### Azure execution on 2026-11-07

The user separately approved two sequential `Standard_D4s_v5` VMs in East US.
Each VM had a 60-minute maximum lifetime. The independent Azure Automation
cleanup control had a 150-minute limit. The total cost ceiling remained USD 10.
All inbound traffic was denied. Payload and evidence blobs remained private.
The cleanup identity had access only to the exact task-owned resource groups.
An actual canary-group deletion verified cleanup before the first VM.

Both guests received the same dirty source snapshot based on
`44a4c67a6bc59d0577d99d42947f6ffbfdbacd1e`. The 18,499,365-byte payload
archive had SHA-256
`528e1cfc1129caab4c7117343f0678c968bef33615b06116c0a2800be5bd2df0`.
It included the base Git bundle and the source overlay. These are working-tree
results, not results for a new committed or pushed revision.

The Windows Server 2022 Azure Edition guest used image
`20348.5773.261025`. Its transferred test host had SHA-256
`940aa321d31851a610ce7802c5de9f3a78b3e03c64471bb69c0f1b5f2bbd0346`.
The first native launches failed because `VCRUNTIME140.dll` was missing.
A Microsoft-signed Visual C++ runtime installation then succeeded without
reboot, and the test host loaded. The normal local account reached all six
initialization entry points, but `Get-DiskImage` rejected image inspection
with `CimJob_BrokenCimSession` and `Access denied`. All six cases stopped
before fault execution. No Windows volume case passed. The application
probes were not elevated and the image association guard was not bypassed.
The Windows resource group was verified absent before Ubuntu admission.

The Ubuntu 24.04 x86_64 guest used image `24.04.202610250`. It compiled the
snapshot with the pinned tools. `cargo task check` completed contracts,
rustfmt, Clippy, workspace and harness tests, Prettier, frontend types, and
all 266 frontend tests. The library suite reported 360 passed, zero failed,
and 10 ignored, including the passing 37-case matrix and all three TLS
parents. The media suite reported 104 passed and one ignored installed-service
entry point. The HTTPS suite passed all 37 tests. A separate targeted TLS
run passed all three parents, and the Python wrapper self-test passed all
three checks.

Earlier operator attempts failed before fault execution because of Linux
script line endings, installer ownership, PATH, Git ownership, test-host
selection, nonempty evidence, and discarded image allocation. These are
setup failures, not application recovery failures or passing volume evidence.
Retries used fresh fixture and evidence roots. The original VM and cleanup
deadlines were not extended.

All six explicitly selected Ubuntu volume cases then passed as the normal
local account on ext4 filesystems in fully allocated 128 MiB loop images.
The successful run lasted from 00:55:19 through 01:10:39 UTC.

| Case                     | Verified result                                                                                                                                                                                                        |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Publication full         | Native `ENOSPC` 28 after an interrupted write. The parent terminated the child. Fresh-process recovery preserved the interrupted output and rejected repair.                                                           |
| Receive full             | Native `ENOSPC` 28. Forced termination and fresh-process recovery preserved partial input without adopting an unsealed receipt.                                                                                        |
| Normalization full       | Native `ENOSPC` 28. Forced termination and fresh-process recovery preserved earlier files and completed through a fresh generation-bound run.                                                                          |
| Database filesystem full | SQLite `SQLITE_FULL` 13 on the full disposable filesystem. Forced termination and fresh-process recovery retained four jobs and 20 events, verified integrity, and admitted new work after space release.              |
| Detached backup          | The operator unmounted the exact loop volume without force. A child persisted `backup_pending`. A fresh process completed after the same original volume returned. Primary and staging content remained intact.        |
| Wrong backup volume      | The separately bound replacement occupied the original mount point. Work stayed `backup_pending` without changing replacement content. A fresh process completed after both volumes returned to their original mounts. |

The four full-filesystem result records confirm process termination and recovery
in a new process. The two backup records confirm fresh-process continuation and
the same original root; they are not additional forced-termination cases.
The downloaded evidence includes native errors, descriptors, inventories,
operator steps, and passing result records. Its archive SHA-256 is
`d8e30a498991b50b03a1761f818d5c33a016e55e9e9cbc6b236938971b0c0e9a`.
Generated media, databases, and recovery data remain private and outside Git.

After evidence export, the Ubuntu resource group was deleted. The independent
guard then deleted the control group. Direct Azure queries verified all four
exact run-owned resource groups absent, including the earlier Windows and
canary groups. No lab VM, disk, network, storage account, or Automation account
remains in those groups. Deallocation was not used as cleanup.
The priced estimate remained below USD 2. Final provider charges are not yet
available, so this is not a settled bill or a provider-enforced USD 10 cap.
No application service was installed, no guest was rebooted, and no existing
library or live provider was used.

### Additional Windows inspection retry

The user separately approved one additional Windows VM and its cleanup
resources within the original USD 10 ceiling. This retry used the same East US
region, `Standard_D4s_v5` size, and Windows Server 2022 image. It created no
additional Ubuntu VM. A new scoped cleanup identity and actual canary deletion
proved independent cleanup before VM admission. All inbound access remained
denied. The Windows group had an unchanged deletion-start deadline of
02:31:10 UTC. No retry extended that deadline or the approved 60-minute VM limit.

The normal-user CIM blocker was replaced with the test-only read-only native
inspector described in the [lab procedure](crash-recovery-lab.md). Real guest
queries exposed two format details: Azure marked the backing disk as removable,
and the returned host-volume GUID name had no trailing separator. The inspector
now binds that removable-host exception to the running OS volume's native
identity and a fixed local drive. It validates the complete GUID before adding
the separator. Exact 128-bit backing-file identity comparison remains required.
It declares no disk attach, detach, format, or write API.

The final 19,352,523-byte source archive had SHA-256
`e7f4af79e09ba70800f011fe46ab20112d113c48ca06e8a230dbbbbf1a7acb82`.
Its Windows test host had SHA-256
`5c2aa69cfa4884a8ab43b9a024cfc41984eb5786841966713b113f04743243e6`.
The base revision remained
`44a4c67a6bc59d0577d99d42947f6ffbfdbacd1e`.
These hashes identify a later dirty snapshot, not the earlier Ubuntu payload
or a new committed revision.

The final parameter-free read-only runner passed both required inspection
checks through the actual Rust entry point under the normal local account:

| Check           | Verified result                                                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| Correct image   | Initialization accepted the fixed 128 MiB NTFS VHDX and wrote its bound descriptor.                                                                   |
| Different image | Initialization rejected the other attached VHDX with `The fixture is not on the supplied image.` No descriptor was written for that rejected request. |
| Authority       | The application process used a normal user token. The result explicitly records `elevated=false`.                                                     |

The downloaded preflight archive had SHA-256
`137d21de43a4de11d1315e2593d00502355962f43ed34941cfe821eaa75cf9a2`.
Its result shape, rejection text, and missing rejected descriptor were checked.
Generated state and recovery data remain private and outside Git.

**The six Windows volume fault cases still did not run.** Earlier inspection
guards rejected the first native attempts. The full-case user runner then
stopped before preflight, while Azure reported only a generic `RunAsUser`
error code 1. Its exact startup failure was not established. The later
parameter-free preflight runner passed, but that does not prove full-case
scheduling or recovery. Further fault admission was refused when too little
bounded VM time remained. No fault was admitted just to use the remaining time.
No application probe was elevated.

After private evidence export, the VM group and cleanup control were deleted.
Direct Azure queries verified all three exact retry groups absent at
02:32:07 UTC, including the already deleted canary group. No retry VM, disk,
network, storage account, or Automation account remains. The combined priced
estimate stayed below USD 3. Final provider charges are not yet available.

For the final native-inspector source, `.\dev.ps1 test --lib volume_lab_` passed
four ordinary tests and left seven explicit lab entries ignored.
`.\dev.ps1 check` passed with 361 library tests, 103 ordinary media tests,
37 HTTPS tests, and 266 frontend tests, plus contracts, rustfmt, Clippy,
harnesses, Prettier, and frontend types. The library suite included the
37-case process-crash matrix and all three TLS crash parents.
These are local Windows results, not remote CI or six-case volume acceptance.
A superseded full-check attempt was stopped during the native path correction.
The complete final run passed afterward.

## Approved Windows volume run (2026-11-07)

The user separately approved one more Windows Server 2022 VM in East US:
`Standard_D4s_v5`, a maximum 30-minute VM lifetime, and a maximum 60-minute
cleanup-control lifetime. The additional estimate was below USD 1 within the
unchanged USD 10 total ceiling. A fresh canary deletion verified the independent
cleanup guard before VM admission. All inbound traffic remained blocked.

This run used the same dirty source archive and Windows test host identified
in the inspection retry above. Both payload hashes were checked in the guest.
Normal-user initialization and wrong-image rejection passed before faults.
The application probes were not elevated. Each case used fresh generated roots
and fixed, fully allocated 128 MiB NTFS VHDX images.

| Case                | Verified Windows result                                                                                                                                             |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Publication full    | Passed. Native storage-full code `112`; the owned child was terminated and recovery ran in a fresh process.                                                         |
| Receive full        | Passed. Native storage-full code `112`; the owned child was terminated and recovery ran in a fresh process.                                                         |
| Normalization full  | Passed. Native storage-full code `112`; the owned child was terminated and recovery ran in a fresh process.                                                         |
| Database full       | Passed. `SQLITE_FULL` code `13`; the owned child was terminated and recovery ran in a fresh process. The saved job count was `6` and the last event ID was `22`.    |
| Backup detached     | Passed. Actual detachment retained `backup_pending`. A fresh process completed after the same original root returned. This case does not add a forced termination.  |
| Backup wrong volume | Failed. Both operator steps were acknowledged, but the parent returned native path-not-found code `3` after the original volume returned. No passing result exists. |

The five passing result records, native codes, and fresh-process flags were
checked after download. The database recovery child checks the saved job count
and last event ID before a new admission. These values depend on where actual
filesystem exhaustion occurs; they are not fixed fixture counts.
The six-case evidence archive has SHA-256
`3000fea0264da79cbf0ddd6e4c5c643c547a4a8dc67b1998c1c2a1c24764e18e`.
It preserves the failed wrong-volume case as well as the five passes.
Generated state and recovery data remain private and outside Git.

A separate, fresh-root wrong-volume retry ran within the same VM deadline.
Its operator waited for drive-letter paths before acknowledging changes.
The user command failed with exit `1`. The operator also failed: its evidence
compression attempted to read an in-use `state.db`. No retry archive was
exported. The ARM command outcomes were retained locally. A drive-letter
readiness race remains a hypothesis, not a verified cause or fix.
The original six-case archive was already downloaded and was not overwritten.
No assertion or native identity guard was removed.

VM deletion started before the unchanged deletion-start deadline of
03:53:23 UTC. Direct Azure queries verified all three exact task-owned groups
absent at 03:55:49 UTC, including cleanup control and the previously deleted
canary. No VM, disk, network, storage account, or Automation account remains
from this run. The combined priced estimate remains below USD 4; final provider
charges are not yet available. No additional VM was created and no deadline
was extended.

The Windows wrong-volume case remains incomplete. The broader combined fault
criterion and issue #22 remain open. These are working-tree results, not new
remote CI evidence. Changes remain local and uncommitted. Nothing was pushed
and no follow-up PR was created.

## Local wrong-volume diagnostics (2026-11-07)

Read-only inspection of the saved Windows archive found a passing
`backup_pending` child log and one job still in `backup_pending`.
The archived database passed SQLite integrity and foreign-key checks.
Both operator continuation signals exist. No completion-child log was
preserved. These facts narrow the failure to the parent checks after volume
return or completion-child startup. They do not identify the missing path
or prove a production recovery defect.

A later test-only change adds operation and path context while preserving
the original error source and native code. Restored-volume validation logs
the expected native identities before opening roots and images, then the
observed identities when available. Inventory failures identify the affected
directory or file. Completion-child diagnostics distinguish log creation,
handle duplication, executable lookup, and process creation. Parent logs
separate volume validation, backup inventory, child startup, and completion
verification.

Ordinary regression tests cover nested native-error preservation, a missing
inventory directory, and a missing child-log parent. The root-binding guard
also checks that identity-conflict errors include the expected and observed
identities. These tests use fresh temporary directories without mounting,
detaching, or filling a workstation volume.

The updated `.\dev.ps1 check` passed with 364 library tests, 103 ordinary
media tests, 37 HTTPS tests, and 266 frontend tests, plus contracts, rustfmt,
Clippy, harnesses, Prettier, and frontend types. The library run included
all seven ordinary volume-lab tests, the 37-case process-crash matrix, and
the three TLS crash parents. The seven explicit volume-lab entries remained
ignored. These are local results, not a new Windows volume acceptance run.

The saved Azure source and test-host hashes above predate these diagnostics.
No new Azure VM or real-volume retest was performed. A future approved guest
run must rebuild and hash its payload. Production recovery behavior and all
identity, content, and approval checks remain unchanged. The Windows
wrong-volume case remains unresolved.

## Targeted Windows diagnosis and recovery (2026-11-07)

The user approved one more temporary Windows Server 2022 `Standard_D4s_v5`
VM in East US, with a maximum 30-minute VM lifetime, maximum 60-minute
cleanup-control lifetime, and an additional estimate below USD 1 within the
unchanged USD 10 ceiling. The current Windows consumption rate was verified
as USD 0.376 per hour. Actual canary deletion verified a fresh independent
cleanup guard before VM admission. Inbound access remained blocked.
Only the wrong-volume case was selected.

The 19,368,247-byte diagnostic source archive, based on the same dirty
`44a4c67a6bc59d0577d99d42947f6ffbfdbacd1e` checkout, had SHA-256
`6a42ceb59b93477fe496c111a642a76b3a90f76fccc1dfe802c19b705f1f5299`.
The rebuilt Windows test-host SHA-256 was
`004bd682815f3024f0014ab72b571ea04682336ea667745eacfcafa520f27f04`.
The guest verified both hashes. Normal-user positive initialization and exact
wrong-image rejection passed.

### Confirmed cause

The failure was in the privileged lab operator procedure, not application
recovery. Its return step selected partitions with
`Where-Object Type -eq Basic`. The original fixture partition reported `IFS`.
The filter selected no partition, so the operator did not restore `V:`.
Recorded mount evidence showed the original at `E:`, the replacement at `W:`,
and `V:\fixture` absent. This was not a path-readiness delay.

The operator now retains the original partition number after verifying its
association with the exact approved image. On return, it queries that same
image's current disk and selects the recorded partition number without a
partition-type filter. Boot/system disk, occupied-letter, fixture-path,
native identity, and content checks remain active.

### Verified results

The first diagnostic attempt refused to acknowledge the incorrect return.
Its separate operator archive was downloaded with SHA-256
`5f49a6a05ce11ec000001da698b599e7bf9383ca371855522a8c36b09eba6cd4`.
After the exact recorded partition was returned from `E:` to `V:` within the
parent's existing wait, the normal-user test completed successfully.
That user archive has SHA-256
`53f4c06e7cc0cd073c78e7ae797c3c951858177371c321aa9e8e21ea436571a3`.
The original operator failure remains recorded as a failure.

A second attempt used fresh roots, fresh images, a fresh descriptor, and a
separate run identity in the same VM. With the corrected operator procedure,
both Azure commands finished `Succeeded` with exit `0`. No manual repair was
used in this repeat. Its result explicitly records `status=passed`,
`fresh_process_continuation=true`, and `same_original_root=true`.
It checked the original and replacement native identities, preserved
`backup_pending` while the wrong volume was present, completed in a fresh
process after return, preserved replacement content, and checked database
integrity. The application token was not elevated.

The independent user and operator archives for the automated repeat contained
the same bounded evidence and both had SHA-256
`13061b5f1440fdf977f5c822123dedf8110d41afe298e9919339784d22753f29`.
The exporter excludes database and media files. Its local safety test proved
that an exclusively locked generated database does not block log export.
No application recovery code or approval rule changed. The full local check
for the diagnostic source remains the 364-library-test result recorded above.
Later repository edits are documentation-only.

After both repeat archives were downloaded and their result shapes were
checked, VM deletion started at 04:53 UTC, before the unchanged 05:06:33 UTC
deletion-start deadline. Direct Azure queries verified all three exact
task-owned groups absent at 04:56:53 UTC. This includes the VM, disks,
network, private storage, cleanup control, and the already deleted canary.
No deadline was extended and no second VM was created for the repeat.
The combined priced estimate remains below USD 5 within the original USD 10
ceiling. Final provider charges are not yet available.

Together with the five earlier passing Windows cases, all six Windows
virtual-volume cases now have passing evidence. Ubuntu's six passing cases
remain separate evidence for its earlier source snapshot. These are not new
remote CI results and do not prove physical drive or power-loss durability.

## Review guard corrections (2026-11-07)

Follow-up review found that a renamed fixture directory could satisfy the
detached-backup path check while its image remained mounted. The parent now
requires read-only native evidence before starting the pending child. Windows
checks the exact VHDX's `GET_VIRTUAL_DISK_INFO_IS_LOADED` state. Linux compares
the image's associated loop-device identities with the kernel mount table,
including mounts at other paths. The bound image identity is checked before
and after inspection. Successful inspection writes `detachment.json`.

Both wrappers now sync and close a temporary sibling before publishing the
continuation signal without replacing an earlier signal. This prevents a
polling parent from reading incomplete content or encountering the Windows
writer's exclusive sharing mode. Ordinary tests cover mounted-image rejection
after a directory rename, malformed observations, hidden writes, failed sync,
and preservation of an existing signal.

The updated local Windows `.\dev.ps1 check` passed with 365 library tests,
103 ordinary media tests, 37 HTTPS tests, and 266 frontend tests, plus
contracts, rustfmt, Clippy, harnesses, Prettier, and frontend types.
Both wrapper safety suites also passed, including all five Python checks.

These corrections postdate the recorded Azure payloads. No new VM or
real-volume retest was performed. The earlier guest results remain evidence
for their stated snapshots, not for these later guard changes.

## Verified PR #59 final merge evidence

Evidence review on 2026-11-08 confirmed that
[PR #59](https://github.com/PlagueHO/domus-obscura/pull/59) merged the TLS and
disposable-volume follow-up as
`75f3c2edd7a24f4b1d4d01f25ba2402c561d8fde`. The
[final merge workflow](https://github.com/PlagueHO/domus-obscura/actions/runs/42345983720)
completed successfully. Both check/build checkout logs identify that exact
revision. These are final merge results, not PR merge-revision results or
new local test results.

| Platform     | Evidence                                                                                               | Verified result                                                                                                                                           |
| ------------ | ------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Windows      | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42345983720/job/128323708301) | 365 library tests passed, zero failed, and ten ignored. Crash/fault parents, all three TLS crash parents, wrapper checks, check, build, and smoke passed. |
| Ubuntu 24.04 | [Check and build](https://github.com/PlagueHO/domus-obscura/actions/runs/42345983720/job/128323708423) | 364 library tests passed, zero failed, and ten ignored. Crash/fault parents, all three TLS crash parents, wrapper checks, check, build, and smoke passed. |

Both logs explicitly report a passing `cross_process_crash_recovery_matrix`
and `cross_process_recovery_rejects_changed_approved_content`. These cover the
37-case matrix and both changed-content cases defined above. The unavailable
backup, simulated publication-full, SQLite-full atomicity, and native ownership
parents also passed. Windows additionally passed sealed-input sharing denial.

All three `backend_configuration_tls_crash_*` parents passed on each platform:
manual reload boundaries, invalid manual material across restart, and managed
rejection with journal/generation preservation. These results do not prove
successful managed production activation.

Ordinary volume-lab guards passed, including rejection of a directory rename
as detachment evidence. The Windows wrapper reported passing rejection and
signal checks without host configuration changes. The Linux wrapper passed
all five self-tests. Each platform ran its own wrapper; the other platform's
wrapper step was intentionally skipped.

The ten ignored library entries include the parent-owned album and TLS child
entry points and seven opt-in volume-lab entries. The child entries are used
by passing parents. The seven lab entries are initialization and six explicit
fault cases; ordinary CI did not run those volume faults. Both separate HTTPS
suites passed 37 tests, and both frontend suites passed 266 tests. Both
pagination jobs passed separately, but they do not prove crash recovery.

No new local Rust, virtual-volume, live-provider, installed-service, or physical
fault test was performed during this review. The earlier guest hashes and
cleanup records remain historical evidence. This final merge includes the later
detachment and signal guard corrections; its ordinary guard checks do not
replace a real-volume retest.

## Accepted scope for issue #22 (2026-11-08)

The user selected process-crash and disposable virtual-volume recovery with
explicit exclusions. Issue #22 requires process-termination recovery with
generated state, controlled provider/TLS failures, and disposable virtual-volume
faults on Windows and Ubuntu. Physical faults are not substitutes for these
required checks, and they are not required to close this issue.

The remaining closure requirement is a repeat of all six volume cases on both
platforms with the current detachment and signal guards. Record the tested
source revision and payload hashes, actual native faults, exact-root return,
fresh-process continuation, preserved content, and database integrity.
Retain the existing process-crash and controlled-fault evidence. Recheck affected
behavior if relevant application or harness code changes.

Successful managed production certificate issuance/activation, physical drive
loss, physical host-volume exhaustion, OS crashes, and power loss are excluded
from #22. Installed-service supervision and reboot acceptance remain under
[#17](https://github.com/PlagueHO/domus-obscura/issues/17) and
[#18](https://github.com/PlagueHO/domus-obscura/issues/18). These are unverified
limits, not passing results. Live-provider execution is not required.

This scope decision supersedes earlier descriptions that treated the excluded
checks as closure requirements. It does not change historical evidence or
authorize resources, host changes, or new lab execution.

## Bounded current-guard retest outcome (2026-11-08)

The separately approved retest uses the unchanged application and test code at
`75f3c2edd7a24f4b1d4d01f25ba2402c561d8fde`. Windows completed exact-image
initialization and wrong-image rejection under the normal account. Its
`publication_full` and `receive_full` cases passed with native Windows error
`112`, forced child termination, fresh-process recovery, and unchanged
interrupted file identities, paths, lengths, and hashes.

`normalization_full` recorded native error `112`, but recovery was interrupted
to preserve the original VM deadline. It is not a pass. `database_full`,
`backup_detached`, and `backup_wrong_volume` did not run. The full-volume cases
took about five minutes each because fresh-process recovery waited for saved
lease expiry. Leases, clocks, native guards, and partial outputs were not
changed to obtain a result.

The Windows evidence archive has SHA-256
`186bf90059330e736ee654da1c5a68b6af68cf8fa26fa89144bdc0c12d53505c`.
The test host has SHA-256
`91e1bce50d7274ddbfa39b25ae5ef1503a8a0956088b6f0e54a017d073605dd3`.
The exact source archive has SHA-256
`f8c485224d9dcfff1157e60fc0411426e0f30ca20adfab2fa6393a6d395dcb99`.
The original payload remains unchanged at SHA-256
`90d8e774371b5b9b6f16966d633b0cad495685eb48cc0a88517e0f4d0e82cd1f`.
Real shallow checkout metadata has SHA-256
`dc2deecda7493ce19cc5c18b2fd6420219bcb5e2295fc789e9562941802ec600`.
Private control evidence retains the actual inline script hashes and failed
setup attempts.

Windows preparation required corrected private download expiry, a private
normal-user temporary directory, real Git metadata and a portable Git runtime,
display-only error normalization, valid run identifiers, and explicit native
exit-code checks. No repository application or harness rule changed.

The four-minute deletion poll timed out. Later authoritative reads verified
Windows group absence before Ubuntu admission at `2026-11-07T20:41:09Z` and
before the Windows deadline at `2026-11-07T20:41:20Z`. The independent cleanup
job kept its unchanged end at `2026-11-07T22:06:11Z`.
The Ubuntu deadline stayed `2026-11-07T21:41:09Z`.

Ubuntu setup and normal-user exact-image preflight passed. The built test host
has SHA-256
`524681ffd3e55f5bbcde5f401f0459a0192a5340ccc26c5a295a04a7dfbeb778`.
All six separate case commands were submitted after preflight. Their final
states and result files were not collected before the session waited for
additional Windows approval. The independent cleanup deadline expired during
that wait. No Ubuntu evidence archive was exported or retained locally.
The guest and private storage are now deleted, so all six Ubuntu fault outcomes
remain unverified. Setup and command admission are not passing fault evidence.

The extra Windows approval arrived after its admission window expired.
No extra VM was created. No deadline was extended. At
`2026-11-08T06:29:39Z`, authoritative operator access verified that all four
original groups and the proposed extra group were absent. A separate resource
inventory found no resources in those groups. This includes the VMs, disks,
networks, private storage, and Automation account. The exact cleanup times and
final cleanup job streams were not retained, so deadline compliance for Ubuntu
and control deletion is unverified. Final provider charges remain unsettled.

Future runs must export evidence before any approval wait. Complete and remove
the active platform first. A new run needs fresh resource, time, cleanup, and
cost approval; the expired approval does not authorize a new control lifetime.

Issue #22 remains open. Its current-guard criterion is not complete. The
existing excluded checks remain outside its accepted scope.

## Additional bounded retest run 1109 (2026-11-08)

The retest used the same clean source revision,
`75f3c2edd7a24f4b1d4d01f25ba2402c561d8fde`, on Ubuntu 24.04 with ext4. The
partial evidence archive passed the partial verifier. It confirms:

| Case                  | Result                                                                                                                                            |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `publication_full`    | Passed with native full error 28 and fresh-process recovery.                                                                                      |
| `receive_full`        | Passed with native full error 28 and fresh-process recovery.                                                                                      |
| `normalization_full`  | Passed with native full error 28 and fresh-process recovery.                                                                                      |
| `database_full`       | Passed with SQLite disk-full error 13 and fresh-process recovery.                                                                                 |
| `backup_detached`     | Interrupted. The operator detached the volume, then the test stopped with `PermissionDenied` (OS error 13) before requesting the original volume. |
| `backup_wrong_volume` | Not performed. Initialization found that its fixture root was not empty.                                                                          |

The detached-backup error was traced to the Linux wrapper. The privileged
operator created its continuation signal with mode `0600`, so the normal probe
account could not read it. The wrapper now sets the signal mode to `0644`
inside the private evidence directory and tests that permission boundary.
The wrong-volume fixture retained data from an earlier attempt. A retry must
use new images, roots, and an evidence directory; do not clear or reuse that
volume.

The archive SHA-256 is
`56035a4d748f4adb9363dcc3fada978f367348c329ba222de0aba118bd8887a4`. The test
host SHA-256 is
`0f38c1b50aee3eb71b420a39b00df3931d8f673b1d2b2d62923288778aeef301`.
Windows remained blocked at the normal-user preflight, so no Windows volume
case passed. The exact Ubuntu resource group was deleted and its absence
verified. The independent cleanup job remained active after this export.

This run does not meet the six-case requirement on either platform. It does not
change the accepted scope or the earlier snapshot-specific results. Keep issue
#22 open until all six current-guard cases pass on Windows and Ubuntu.

## Windows launch diagnostic run 1110 (2026-11-09)

The user approved one Windows diagnostic VM to resolve the normal-user launch
blocker. This approval covered a launch diagnostic only, not a volume-fault
case. Managed Run Command `RunAsUser` again returned `Incorrect function`,
although Secondary Logon was running. Task Scheduler S4U registration returned
`Access is denied`.

A bounded diagnostic used `LogonUser` with batch logon and
[`CreateProcessWithTokenW`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-createprocesswithtokenw)
to launch `cmd.exe` and list the test host's lab entry. The process ran as
`domus-windows\domusprobe`, with `elevated=false`; the command exited `0`.
This verifies the test-host launch path only. It does not verify lab
initialization, fixture access, or any of the six recovery outcomes.

The diagnostic evidence archive SHA-256 is
`d7f042ab59a10ef4a35534dafb38ef256f22abf48adbfbb315fe46f093a40fa2`.
After the archive was downloaded, direct Azure checks confirmed all four exact
run 1110 resource groups absent. This removed the VM, cleanup control, private
storage, and Automation resources. No volume-fault case ran. Issue #22 remains
open. A full current-guard retest still needs separate bounded approval and
must cover Windows NTFS and Ubuntu ext4.

## Bounded current-guard retest run 1111 (2026-11-09)

The approved run used fresh disposable Azure guests and the fixed cleanup
window. Windows setup and volume preparation succeeded, but the normal-user
preflight did not pass. The first preflight stopped because `domusprobe` was
not a standard `Users` member. After that membership was corrected, Git's
`SYSTEM`-account check still rejected the probe-owned checkout. The runner was
changed so Git checks run as `domusprobe`, but the Windows deadline expired
before that fix could be tested. No Windows recovery case ran.

Ubuntu setup, exact-image preflight, and all six normal-user recovery cases
completed. Each case result reported `passed`, and the exported evidence
verifier checked the native full errors, recovery assertions, and backup
continuation evidence:

| Case                  | Observed Ubuntu result                                                  |
| --------------------- | ----------------------------------------------------------------------- |
| `publication_full`    | Passed; native full error 28 and fresh-process recovery.                |
| `receive_full`        | Passed; native full error 28 and fresh-process recovery.                |
| `normalization_full`  | Passed; native full error 28 and fresh-process recovery.                |
| `database_full`       | Passed; SQLite disk-full error 13 and fresh-process recovery.           |
| `backup_detached`     | Passed; fresh-process continuation after the original backup returned.  |
| `backup_wrong_volume` | Passed; pending state was preserved until the original backup returned. |

These are not accepted as issue-closure evidence. Every `case-started.json`
reported `dirty=true`; the guest's Git status identified the modified
`scripts/crash-recovery-lab.py`. The run-specific verifier therefore reported
`performed_not_accepted`, not `passed`. The source archive SHA-256 is
`ecf0ae6a3957b1c2169aacaa1d0c8c2b9c3d6d4056e44826f54195dccb89d857`, the test
host SHA-256 is
`0079d8b5ee4a638f4f8bc953d106c186f89aa244ac9fb33d00424c3beb46b159`, and the
exported Ubuntu evidence SHA-256 is
`ab767e529124dff77038b566f79a8e02aff8388cecf44ec16e3413fd9a581858`.

The Windows, Ubuntu, and canary resource groups were verified absent. The
cleanup job remained running and reported scope denial when it tried to read
the already-absent Windows group. After verifying the exact control-group
owner and all guest-group absences, the operator deleted the exact run-owned
control group. Direct Azure checks then confirmed all four groups absent. This
was operator cleanup, not a successful cleanup-job completion. Keep issue #22
open. A future acceptance run must use a clean, pinned source revision and pass
all six cases on both platforms.

## Acceptance map for issue #22

| Issue criterion                                  | Evidence                                                               | State and limit                                                                                                                                                                                                                                       |
| ------------------------------------------------ | ---------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Terminate/restart at acquisition boundaries      | 37-case matrix in both final merge check/build jobs                    | Verified for process termination, including MP3 and FLAC normalization.                                                                                                                                                                               |
| No duplicate work, stale mutation, lost question | Matrix checks of submissions, claims, and durable questions            | Verified with generated state and a loopback peer, not a live provider.                                                                                                                                                                               |
| Unavailable drives, full storage, locks, DB/TLS  | Controlled faults, historical volume runs, run 1111 results, TLS tests | Open: run 1111's Ubuntu cases used a dirty checkout, and no Windows case ran. A clean-source six-case retest is still required on both platforms. Controlled provider/TLS failures are in scope; production success and physical faults are excluded. |
| Preserve replacement/restore/deletion content    | Matrix plus both changed-approved-content rejection cases              | Verified for process termination. Current, retained, and changed content remain distinct.                                                                                                                                                             |
| Separate process from OS/power-loss evidence     | Explicit boundaries in this record                                     | Verified reporting distinction. No OS/power-loss result is claimed.                                                                                                                                                                                   |
| Record both platforms and privileged limits      | Exact final merge job links and historical guest records               | Verified reporting on Windows and Ubuntu. Unperformed host checks remain explicit.                                                                                                                                                                    |

## Required follow-up and excluded checks

Only the current-guard volume retest in the table below is required to close #22.
The other rows describe evidence that remains unverified and outside its accepted
scope. They can inform separately approved work, but they do not block closure.
This decision does not authorize execution. A later run needs separate approval
for its exact scope and resources. Keep private configuration, keys, media,
databases, and recovery evidence outside Git and public issue text.

| Check                         | Fault and observations if separately approved                                                                                                                                                                                                                                                                                              | Platform and approval boundary                                                                                                                              |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Current-guard volume retest   | Run all six cases with fresh images using the final merge guards. Record actual native full errors and detachment evidence. Check fresh-process recovery, original root identity, unchanged partial/replacement bytes, and SQLite integrity.                                                                                               | Windows NTFS and Ubuntu ext4. Separate bounded disposable-VM and volume-operation approval. Historical passes remain valid only for their snapshots.        |
| Managed production TLS        | Use an exact-hostname publicly trusted production generation without replacing the trust policy. Terminate owned hosts at documented activation transitions. After restart, compare saved journal and immutable generations with the certificate from a fresh guarded TLS handshake. Reject invalid candidates without anonymous fallback. | Windows and Ubuntu. Separate hostname, certificate, identity, provider, and resource approval as needed. Generated manual certificates are insufficient.    |
| Physical drive loss           | Remove an approved disposable physical media/backup device during a documented operation. Record native absence and pending/error state. Return the exact original device and verify fresh-process continuation, root identity, receipts, and unchanged unrelated content.                                                                 | Native Windows and Linux lab hosts. Separate hardware and host-change approval; no existing library or shared disk.                                         |
| Physical host-volume full     | Exhaust an approved isolated physical local filesystem, not a SQLite page limit or virtual image. Record native storage-full/SQLite errors. Restore capacity explicitly and verify rollback, saved jobs/events, partial bytes, receipts, and fresh-process recovery. Keep control and evidence storage healthy.                            | Native Windows and Linux lab hosts. Separate storage and host-change approval with explicit capacity and cleanup bounds.                                    |
| Installed-service supervision | Terminate an exact owned service worker at selected durable boundaries. Observe the actual service manager's restart and restricted-account permissions. Verify fresh ownership, no duplicate submission, durable questions, and preserved current/retained content.                                                                       | Windows service and Linux systemd acceptance under #17 and #18. Separate installation, account, permission, and service approval.                           |
| Reboot startup                | Reboot an approved service lab host with recorded pending work. Verify automatic startup under the intended account and continuation against the same roots/database. Check receipts, questions, identities, and content before admitting new work.                                                                                        | Native service hosts under #17 and #18. Separate service setup and reboot approval; package builds are not startup proof.                                   |
| OS crash                      | Interrupt the operating system at selected durable boundaries in an approved isolated lab. After boot, inspect SQLite integrity, saved intents/receipts, native identities, and all content before explicit continuation. Report actual lost or uncertain writes rather than repairing them to pass.                                       | Windows and Linux OS-specific lab methods. Separate destructive host-fault approval and an independent recovery/evidence path.                              |
| Power loss                    | Cut power to an approved isolated physical storage/host setup at selected boundaries. After return, compare acknowledged durable state, SQLite integrity, identities, and hashes before continuation. Record controller/cache behavior and any uncertainty; do not extrapolate beyond the tested hardware.                                 | Physical Windows and Linux lab configurations. Separate hardware, power-control, and host-change approval; VM termination is not physical power-loss proof. |

Live-provider execution is not added as a requirement or authorized here.
The loopback response-loss evidence must retain that limit. If live-provider
acceptance is requested later, define its credential, download, cost, and
no-duplicate-submission boundaries before execution.

## Remaining acceptance limits

Windows and Linux process-crash evidence is verified for both the PR merge
revision and the final merge revision of PR #56, and for the final merge of
PR #59 above. This does not complete issue #22.
Installed-service supervision, reboot, OS/power-loss, physical drive-loss, and
physical host-volume-full checks were not performed. The controlled faults
retain their limits in the table above. The earlier CI evidence-only review
changed no application behavior or test code. The follow-up adds test-only
probes and lab procedures, with Ubuntu volume evidence recorded above.
Windows normal-user image inspection and all six virtual-volume fault cases
are now verified across the approved runs. The wrong-volume failure was
resolved by correcting the lab operator's partition selection.
Successful managed production TLS activation remains unverified.

Keep issue #22 open until the current-guard volume retest passes on both
platforms and its evidence is recorded. The excluded checks do not block closure.
Native deployment work remains deferred under #17 and #18. Do not treat package
builds or process-crash tests as installed-service or power-loss acceptance.
