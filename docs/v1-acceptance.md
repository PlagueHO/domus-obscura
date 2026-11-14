# V1 acceptance

## Scope

Issue [#24](https://github.com/PlagueHO/domus-obscura/issues/24) uses generated
fixtures only. Live compatibility tests are **not approved and not performed**.
Do not use live credentials, existing libraries, public listeners, or privileged
host changes for these checks. Native deployment remains deferred.

Generated browser tests use the separate `browser_workflow` example, private
temporary roots and SQLite, generated catalog/audio/artwork, and real Rust
planning, normalization, publication, and recovery operations. They do not mount
the production API router or prove production sign-in. Production-router guards
are checked separately with generated HTTPS and identity fixtures.

## Required evidence

The test names below identify checks, not passing results. Record execution
results for the exact tested source revision before changing a row to passed.

| Requirement                             | Check or evidence                                                                                                                                                               | Boundary                                                                                                   |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Locked check/build/smoke                | `dev.ps1 check`, `build`, `smoke`; matching `cargo task` commands in CI                                                                                                         | Local Windows and native Ubuntu results must be separate.                                                  |
| Browser MP3/FLAC complete albums        | `frontend/browser/workflow.spec.ts`, desktop and narrow Chromium                                                                                                                | Generated source, exact format, three tracks across two discs, main and backup.                            |
| Exact output                            | Browser fixture independent ID3/FLAC readers, exact file sets, approved JPEG bytes, encoded-audio checks                                                                        | Every expected verified destination; pending backup is not complete.                                       |
| Production auth/TLS rejection           | `https::browser_sign_in_guards_commands_rotates_sessions_and_preserves_durable_state`, `tls_rejects_invalid_material_wrong_hosts_and_mismatched_keys`; `identity` tests         | Generated identity and trusted local certificate fixtures, not live Entra or managed certificate issuance. |
| Stale answers and changed approvals     | `questions::a_changed_plan_invalidates_an_old_question`, `https::controls::operator_controls_need_guards_and_explicit_answer_then_continue`, browser decision and restore cases | Rejection preserves state; an answer does not authorize new media effects.                                 |
| Backup and retained recovery            | Browser backup-pending, replacement/restore/deletion cases; media publication and recovery tests                                                                                | Main completion and backup completion remain distinct.                                                     |
| Protocol MP3/FLAC and complete decoding | `complete_mp3_and_flac_albums`, media and format tests                                                                                                                          | Generated loopback protocol and files, not a live peer or independent pressing proof.                      |
| Process and volume recovery             | [Recovery acceptance map](crash-recovery.md#acceptance-map-for-issue-22)                                                                                                        | Accepted historical Windows NTFS/Ubuntu ext4 runs, not OS crash, power loss, or installed services.        |
| Physical scan and combined memory       | [Physical scale evidence](physical-scan-scale.md) and separate million-record CI jobs                                                                                           | Accepted selected generated profiles; not the full physical library, SMB, or live transfers.               |

## Execution record

Planning started at `a1316f1843ca84ba7394d9cde1b61ffb122fba61`.
Local and remote results for new acceptance changes are **not yet recorded**.
Earlier CI or a passing source inspection is not current-revision evidence.

Use the pinned tools and the normal launcher:

```powershell
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
.\dev.ps1 browser
```

On native Ubuntu:

```sh
cargo task check
cargo task build
cargo task smoke
cargo task browser
```

Use an already installed, permitted Chromium-family browser with
`DOMUS_BROWSER_EXECUTABLE` when needed. Installing a browser needs separate
approval. Do not disable certificate validation to run a browser.

Retain the source SHA, dirty state, platform, browser version, fixture shape,
commands, exit results, CI job links, and browser report for each run. Keep raw
private state, audio, credentials, and recovery files out of Git and issue text.
Record failed, interrupted, skipped, and unavailable checks explicitly.

## Remaining delivery gates

This snapshot records issue states observed during planning. Refresh the states
and relevant comments/artifacts before the final acceptance decision.

| Gate                            | State                                                                          | Required treatment                                                                                       |
| ------------------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| #22 recovery                    | Closed; accepted scope recorded in the recovery guide and latest issue comment | Preserve the original revisions and excluded checks. An older unchecked body is not the current verdict. |
| #23 physical scale              | Closed; selected generated profiles accepted                                   | Preserve dataset and memory-measurement limits.                                                          |
| #25 security/public exposure    | Open; no comments or artifacts returned                                        | Remains an independent gate. Do not expose publicly or infer security acceptance from guard tests.       |
| #26 dependency provenance       | Open; no comments or artifacts returned                                        | Distribution notices and permission proof remain independent.                                            |
| #30 retained service boundaries | Open; no comments or artifacts returned                                        | Do not add a blanket refactor here or claim adapter parity without its evidence.                         |
| #17/#18 native deployment       | Deferred                                                                       | Not passed. Do not schedule installation, account, permission, service, or reboot work.                  |
| #48 Windows host acceptance     | Open and separately approved only after other work                             | No VM or privileged operation is authorized here.                                                        |
| #27/#28/#29 future work         | Explicitly outside V1                                                          | Do not use these issues to block V1 acceptance.                                                          |

Keep #24 and the V1 tracker open while required in-scope evidence or unresolved
V1 gates remain missing. Generated-fixture completion is not complete V1,
live-provider acceptance, installed-service acceptance, or approval to deploy.
