# Process-crash recovery evidence

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

## Remaining acceptance limits

Windows and Linux process-crash evidence is verified for both the PR merge
revision and the final merge revision above. This does not complete issue #22.
Installed-service supervision, reboot, OS/power-loss, physical drive-loss, and
physical disk-full checks were not performed. The controlled faults retain
their limits in the table above. No installed service, account, mount, live
provider, existing library, or host-volume changes were made during this
evidence review. No application behavior or test code changed.

Keep issue #22 open for the remaining evidence. Native deployment work remains
deferred under #17 and #18. Do not treat package builds or process-crash tests
as installed-service or power-loss acceptance.
