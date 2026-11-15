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

## Security and public-exposure gate (#25)

The security-review agent inspected the integrated application at
`5197892ad2888242f2d42dd1e09030d00c754b58`. A second, narrower review covered
decoder admission, dependency security boundaries, and SQLite locality.
These were source reviews, not live attacks or an exhaustive all-line audit.
The user selected fixes for all confirmed vulnerabilities.

| Finding                                           | Severity | Confidence | Evidence and disposition                                                                                                                                                                                                                                                                                                                                                                                |
| ------------------------------------------------- | -------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Predictable reverse-upload connection credentials | Medium   | 9/10       | Original `crates/domus/src/infrastructure/soulseek/sharing.rs:21-30,628-645,993-1064`. A client behind the intended receiver's public IP could guess a pending counter token and win the file-connection race. Reverse-upload allocation now uses cryptographic randomness, bounded collision-safe reservation, single-use routing and exact-reservation cleanup. Execution evidence is required below. |

The attack required explicitly enabled sharing, a reachable sharing listener,
an authorized reverse upload, a shared public IP, and a successful connection
race. Default disabled sharing and outgoing-only sharing were unaffected.
The fix retains the protocol's 32-bit token width and existing IP, allowlist,
generation and upload-authority checks. Zero tokens are rejected. Reservation
has at most 32 attempts. Random-source failure or reservation exhaustion is
an explicit error, without a predictable fallback. Cleanup compares the exact
reservation so an older upload cannot remove a newer route with the same token.
Its weak cleanup guard does not keep the response sender alive after account
reconnection clears the routes. Waiting uploads still detect disconnection.

The existing Rustls Ring provider supplies the randomness. Its
[SecureRandom contract](https://docs.rs/rustls/0.23.46/rustls/crypto/trait.SecureRandom.html)
requires cryptographically secure bytes and reports random-source failure.
No dependency, schema, shared contract, sharing policy, or listener change is
part of this fix. Soulseek remains a plaintext protocol with noncryptographic
username/address identity. Random tokens do not remove those protocol limits.

### Review coverage

| Area                         | Source evidence                                                                                                                                                               | Required runtime evidence and limits                                                                               |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Identity and sessions        | `identity.rs:197-263`; `store/sessions.rs:51-158`: issuer, signature, audience, nonce, tenant/object identity, hashed sessions and atomic rotation                            | Generated identity/session tests; no live Entra proof                                                              |
| Browser guards               | `api.rs:1142-1302,2692`; `frontend/app/api.ts:107-133`: Host, Origin, CSRF, loopback peers, cookies, stream-session checks and no automatic command retries                   | Production-router rejection fixtures; the separate browser harness is not production sign-in proof                 |
| TLS lifecycle                | `tls.rs:42-104`: material, hostname, validity, chain and key checks; selected managed lifecycle and guarded-listener boundaries                                               | Generated TLS rejection/activation tests; successful managed-production issuance remains separate                  |
| Native paths and permissions | `filesystem.rs:140-234`; `permissions.rs:62-89,166-224`: component-wise no-follow access and actual-handle permissions                                                        | Native Windows/Linux fixture evidence; installed-account and volume checks remain separate                         |
| Cache and staging            | `cache.rs:294-350`; `staging.rs:101-155,390-438`: native ownership, private files, exact completion and sealed receipts                                                       | Ownership, copied-marker, interrupted-stream and receipt checks                                                    |
| Database and recovery        | Selected `store.rs:184-202,237-305` queries; `application/recovery.rs:109-157,385-468`: fenced claims, transactional state and exact-state recovery approval                  | Stale claims, changed approvals, interrupted operations and backup-pending checks; not every SQL query was audited |
| Providers and local MCP      | `caa/transport.rs:148-207`; `mcp.rs:359-431,491-635`: bound redirect authority, typed bounded local tools and cancellation                                                    | Generated protocol/input/limit fixtures; no live providers or public MCP                                           |
| Worker and sharing           | `production_worker.rs:298-315,509-607`: process ownership, frozen configuration and explicit decisions; finding above                                                         | Preserve uncertain submissions, approvals and receipts; isolated sharing tests                                     |
| Audio and images             | `mp3.rs:219`; `flac.rs:79`; `artwork.rs:14`; `artwork/jpeg.rs:11`; `artwork/color.rs:19`: bounded admission, complete decode, independent checks and native warning rejection | Decoder rejection fixtures; no fuzzing, sanitizers, OS decoder isolation or total-process memory proof             |
| Public exposure              | `server/configuration.rs:1457`: non-loopback API listener rejection                                                                                                           | Keep rejection in place; separate explicit deployment/exposure decision                                            |

Rust source paths in this table are relative to
`crates/domus/src/infrastructure` unless an application path is specified.
Line references identify the reviewed baseline, not later shifted lines.

### Dependency and storage limits

The narrow dependency review inspected selected pinned versions and disabled
default features. It queried OSV with 32 public package/version pairs only.
It did not transmit source or lockfile contents. This is not a complete
dependency audit or proof that no dependency has a vulnerability.
`cargo-audit` was unavailable and was not installed.

The version match for
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html)
affects bundled SQLite. Its identified attack needs an attacker-written
database and sibling WAL. No such input path was established under supported
operator trust. Keep database, WAL and recovery snapshots under trusted
operator control. Do not accept an arbitrary database as provider input.
The RSA private-key timing advisory
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
does not establish an attack on the inspected signature-verification use.
The Hickory DNSSEC advisory
[RUSTSEC-2026-0514](https://rustsec.org/advisories/RUSTSEC-2026-0514.html)
requires DNSSEC validation; the selected manifest does not enable that feature.
These are recorded dependency risks and applicability limits, not additional
confirmed application exploits. Dependency provenance remains #26.

SQLite must remain on local disk. `filesystem.rs:22-37` checks path syntax,
and Store repeats this check after canonicalization. This does not prove
filesystem locality for mapped Windows drives or Linux network mounts.
The operator must independently verify local storage. No untrusted placement
path was established in this review. Keep this enforcement limit visible;
do not claim that the current guard detects every network filesystem.

Restricted installed-account permissions, service/reboot acceptance, physical
crash/power-loss proof, live Entra, managed-production TLS activation and
live-peer compatibility remain separate evidence gates. Linux recovery also
retains its documented cooperative-writer boundary. Restoring an older SQLite
snapshot does not roll back external submissions or certificate/DNS effects.

### Security execution record

Local verification used uncommitted application changes on top of
`5197892ad2888242f2d42dd1e09030d00c754b58`. Only `sharing.rs` changes application
code. Its tested SHA-256 is
`7a9dc6897eb44429477fa940707ebaebbeee1ce376a67ea2d114b58561499893`.
The other change is this acceptance document. This execution record describes
the tested source before publication; it is not a commit, push, PR, merge or
remote CI result.

| Current-source local Windows check        | Result                                                                                                                                       |
| ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Sharing tests within the final full check | Passed: all five new reverse-connection regression cases and existing upload, authority and disabled-sharing checks.                         |
| `.\dev.ps1 check`                         | Passed: contracts, rustfmt, denied-warning Clippy, 762 Rust tests including both harnesses, Prettier, frontend types and 266 frontend tests. |
| `.\dev.ps1 build`                         | Passed: frontend builds and locked Rust application build.                                                                                   |
| `.\dev.ps1 smoke`                         | Passed: `Domus Obscura 0.1.0`.                                                                                                               |

The first full check rejected `unwrap()` in the new test fixtures. The fixtures
now return explicit errors, and the final full check passed. The existing
ts-rs `deny_unknown_fields` parse diagnostics remain; contract checks passed.
The final full check reported 21 ignored Rust entry points. Private child
entry points run through their parent tests. Separate operator volume,
installed-service, full-transfer and million-record cases are not new passing
results. Index and query behavior did not change, so no new scale run was
required.

The five new cases check collision/zero rejection, random-source failure and
bounded exhaustion, concurrent reservations, same-IP unknown-token rejection,
single-use replay, changed policy/IP/generation, exact-reservation cleanup and
response-channel disconnection when routes are cleared.
Existing upload-offset, lost-authority, unchanged-source and disabled-sharing
cases also passed. These are generated fixtures, not a live protocol attack.
The broader full suite rechecks identity, browser commands, TLS, native roots,
cache/staging ownership, provider/MCP limits and approval/recovery rejection.

Environment: Windows x86_64, OS build `10.0.26100`; Rust `1.99.1`, Node
`v26.12.1`, pnpm `12.15.0`, and launcher-built pinned libjpeg-turbo `3.2.0`.
Command logs are retained in the session artifact storage. The final
documentation update changes no application code.

### Native Linux backend verification

The initial shell did not find Cargo on its `PATH`. Further tool discovery
found the installed native Rust `1.99.1` toolchain. No installation was needed.
The final backend checks used Ubuntu `24.04.5` x86_64 on
`6.18.58.1-microsoft-standard-WSL2`, with source and fixtures on local ext4.
This is Linux backend evidence, not installed-service or reboot proof.

A tracked-source archive at the reviewed baseline, with the local source
changes applied, supplied the isolated checkout. Its `sharing.rs` SHA-256
matched the Windows value above before and after the run. The first full
library run failed two lab-boundary fixtures because the source archive
omitted Git metadata. A local Git repository was initialized in the generated
snapshot, without a commit or remote. The two fixtures and the final full
Rust suite then passed. No test or application rule was weakened.

| Current-source Linux backend check                     | Result                                                                                                          |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `cargo task test sharing`                              | Passed: sharing checks, including all five new regression cases.                                                |
| `cargo task test`                                      | Passed: full workspace Rust suite.                                                                              |
| `cargo fmt --all --check` and denied-warning Clippy    | Passed. Clippy used the task-built pinned native JPEG library with the normal explicit/static binding settings. |
| Rust `local_test` and `browser_workflow` example tests | Passed: 9 and 8 tests. These are Rust harness tests, not browser-driven MP3/FLAC acceptance.                    |
| Rust `contracts-check`                                 | Passed.                                                                                                         |
| `cargo task smoke`                                     | Passed: native Linux application build and `Domus Obscura 0.1.0`.                                               |

The final workspace and example runs passed **750 Rust tests**, with **21
ignored entry points**. This total excludes the preliminary targeted fixture
recheck and the earlier focused sharing run. Logs are retained with the Windows
evidence. No application source changed after the Windows checks.

**Combined Linux frontend checks remain unavailable.** Native Node.js is not
installed. `cargo task check` and `cargo task build` both stopped at the
toolchain check with `No such file or directory (os error 2)` and exit code 1.
The backend checks above do not substitute for those full tasks. No tool
installation was authorized or performed. This local execution record does not
include remote CI results for the publication commit. Keep full Linux
frontend/browser acceptance in #24.

At the reviewed baseline, the
[Ubuntu check/build job](https://github.com/PlagueHO/domus-obscura/actions/runs/43644058588/job/132597773965)
passed. The
[Windows check/build job](https://github.com/PlagueHO/domus-obscura/actions/runs/43644058588/job/132597773838)
failed in the existing narrow-browser metadata-error test because its alert
locator matched two elements. That browser test was not changed or rerun here.
The baseline
[CodeQL run](https://github.com/PlagueHO/domus-obscura/actions/runs/43644058583)
passed. These are historical results, not verification of the new patch.
The wider browser and current-revision Windows/Linux CI matrix remains #24.

**The security review and local fixes are complete within the recorded scope.**
Security/public acceptance remains open. The fix does not enable public binding
or authorize deployment. Missing full Linux frontend, live integration or
installed-service evidence must not be reported as passed.

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
The implementation is committed locally as
`4b7eaee3a922a7ec44a4387a57d5e1d6febd2d82`. The final local checks below used
that clean revision. The later acceptance-record update changes documentation
only; it is not a new application-behavior result.

| Local Windows check         | Result                                                                                                                                                     |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.\dev.ps1 check`           | Passed: contract check, rustfmt, denied-warning Clippy, 757 Rust tests including both harnesses, Prettier, frontend types, and 266 frontend tests.         |
| `.\dev.ps1 build`           | Passed: frontend builds and locked Rust application build.                                                                                                 |
| `.\dev.ps1 smoke`           | Passed: `Domus Obscura 0.1.0`.                                                                                                                             |
| `.\dev.ps1 browser`         | Passed: 37 desktop/narrow checks, zero unexpected or flaky outcomes, and three expected project-specific layout skips.                                     |
| Browser fixture regressions | All seven passed, including missing/extra files, changed audio/tags/JPEG, incomplete destinations, empty reset compatibility, and invalid fixture options. |

Environment: Windows x86_64, OS build `10.0.26100`; Rust `1.99.1`, Node
`v26.12.1`, pnpm `12.15.0`, and existing Chrome `157.0.8103.79`.
The browser used one worker, desktop `1440x1000`, and narrow `390x844` with
reduced motion. Complete-album fixtures contain three silent generated tracks
across two discs. They test main-only publication and main-plus-backup
publication with an unavailable backup and explicit continuation.

The browser report records source revision `4b7eaee3a922a7ec44a4387a57d5e1d6febd2d82`
and `dirty=false`. Its SHA-256 is
`6a5010134a94a159eab0290cf0ec8a9072943fe929e117099c7427436937f2a6`.
The report and four command logs are retained in the session's artifact storage.
The normal ignored report remains `frontend/test-results/browser-results.json`.
CI uses the [built-in JSON reporter](https://playwright.dev/docs/test-reporters#json-reporter)
and retains success/failure evidence under `browser-results-{OS}-{SHA}`.

The full check also reported **21 ignored Rust entry points**. Private child
entry points run only through their owning parent checks. Operator volume
cases, the service exporter, the separate full-transfer workload, and six
million-record scale cases were not independently run by this command. Index
and query behavior did not change, so no new scale run was required. Preserve
the separate historical scale and recovery evidence rather than marking these
ignored entries passed.

**Current-revision native Linux and remote CI evidence remains unverified.**
The implementation has not been pushed, and no PR was created. No remote run
was started for this commit. Do not infer Linux success from local Windows.

The starting revision's
[Windows check/build job](https://github.com/PlagueHO/domus-obscura/actions/runs/43628385423/job/132549293402)
and
[Ubuntu 24.04 check/build job](https://github.com/PlagueHO/domus-obscura/actions/runs/43628385423/job/132549293406)
passed. Its separate
[Windows million-record job](https://github.com/PlagueHO/domus-obscura/actions/runs/43628385423/job/132549293366)
and
[Linux million-record job](https://github.com/PlagueHO/domus-obscura/actions/runs/43628385423/job/132549293180)
also passed. These are historical results at `a1316f1843ca84ba7394d9cde1b61ffb122fba61`,
not evidence for the new browser cases.

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

The issue states were checked again during this work. Tracker #1 now marks
closed implementation issues #3 through #16 and #19 through #23. Its legend
states that issue closure is not independent acceptance evidence. It preserves
the open and deferred gates and the future-work exclusions. Refresh states and
relevant comments/artifacts before any later final acceptance decision.

| Gate                            | State                                                                          | Required treatment                                                                                            |
| ------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| #22 recovery                    | Closed; accepted scope recorded in the recovery guide and latest issue comment | Preserve the original revisions and excluded checks. An older unchecked body is not the current verdict.      |
| #23 physical scale              | Closed; selected generated profiles accepted                                   | Preserve dataset and memory-measurement limits.                                                               |
| #25 security/public exposure    | Open; source review and local fix verified on Windows and Linux backend        | Full Linux frontend and separate operational acceptance remain limited above. No public exposure is approved. |
| #26 dependency provenance       | Open; no comments or artifacts returned                                        | Distribution notices and permission proof remain independent.                                                 |
| #30 retained service boundaries | Open; no comments or artifacts returned                                        | Do not add a blanket refactor here or claim adapter parity without its evidence.                              |
| #17/#18 native deployment       | Deferred                                                                       | Not passed. Do not schedule installation, account, permission, service, or reboot work.                       |
| #48 Windows host acceptance     | Open and separately approved only after other work                             | No VM or privileged operation is authorized here.                                                             |
| #27/#28/#29 future work         | Explicitly outside V1                                                          | Do not use these issues to block V1 acceptance.                                                               |

Keep #24 and the V1 tracker open while required in-scope evidence or unresolved
V1 gates remain missing. Generated-fixture completion is not complete V1,
live-provider acceptance, installed-service acceptance, or approval to deploy.
