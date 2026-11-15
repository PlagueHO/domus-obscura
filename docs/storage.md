# Shared storage client: contract and relationship experiments

## Status

This is the first-stage design for [#68](https://github.com/PlagueHO/domus-obscura/issues/68).
The user requires prototype evidence and separate design approval before
production changes.

The user selected one private application directory with overridable child
folders and separate primary/backup media libraries. Media can be local or on
network file shares; SQLite must remain local. See the
[application storage layout](storage-layout.md) for the seven confirmed policy
decisions, including default paths, read-only host configuration, coordinated
restart, and explicit fresh setup. Detailed configuration contracts and safe
mount arrangements still need implementation and verification.

**The simple retained-boundary proposal failed its alias-separation gate.**
Removing container-specific ancestors makes identity comparison stable, but does
not prove that media and private storage are separate. Do not use the experiment
to authorize storage. No production binding semantics, configuration fields, or
database migrations change in this stage.

**A restricted replacement now rejects the real hidden alias.** It correlates
retained Linux directory handles with kernel mount-root coordinates. Generated
same-ext4 fixtures pass sibling separation, alias rejection, descendant-mount
refusal, and identity comparison across side-by-side/recreated containers.
This is not approval for runtime integration or proof for arbitrary mounts.
Network media, different filesystems, native Linux hosts, and Windows containers
still need separate evidence. See [replacement proof](#replacement-proof).

Existing native operation and explicit legacy-binding commands keep their
current behavior. The agreed container layout still requires a valid binding
design and separate platform verification.

## Client contract

Extend the existing filesystem adapter rather than adding a storage service.
Use one concrete Rust client per process. Add a trait only at a demonstrated
I/O or testing boundary.

| Responsibility                                                                                                       | Owner                                    |
| -------------------------------------------------------------------------------------------------------------------- | ---------------------------------------- |
| Open each component without following links; retain handles; collect native identities; establish root relationships | Shared filesystem client                 |
| Save and query root records, versions, observations, and receipts                                                    | Existing SQLite `Store`                  |
| Coordinate inspection, registration, explicit migration, claims, approvals, and effects                              | Application use cases                    |
| Select configured roots and construct the client                                                                     | API, worker, and CLI composition         |
| Normalize audio, decode images, and enforce provider rules                                                           | Existing adapters and application policy |

The client must not own a SQLite connection, execute SQL, approve a media
destination, acquire a job claim, or contact a provider. Consumers must not
reimplement its filesystem checks.

Keep these operations distinct:

1. **Inspect:** return bounded evidence or a typed error without changing state.
2. **Open an existing binding:** verify saved authority and return retained
   handles for the requested operation.
3. **Register:** inspect existing directories and save an explicitly admitted
   root. Do not create or move media.
4. **Migrate a binding:** require a separate exact-state preview and confirmation.
   Startup and configuration save cannot perform this operation.

Native paths come only from operator configuration. Domain requests use
`LibraryPath` or saved entity IDs. Do not provide a generic path-based bypass.
Opening a library for reading does not authorize publication, replacement,
restore, deletion, or sharing.

Read saved records before filesystem work. Perform filesystem I/O outside SQL
transactions and async executor threads. Commit evidence with expected versions
in a short transaction. Recheck the root inventory inside registration commits
so concurrent registrations cannot each admit conflicting roots. Retain the
verified handles through I/O; never restore checked-path-then-open access.

## Layout and root relationships

Pre-production breaking changes are permitted. Backward compatibility is not
required. Report incompatible state explicitly and allow fresh setup without
moving or deleting old state or media. Media remains separately configured.
The exact new configuration schema is not approved.

The candidate layout places private state, cache, and staging in separate
children of a retained boundary. Existing media can be mounted at separately
declared locations. Placement below that boundary does not itself prove safe
separation.

| Root                       | Required relationship and ownership                                                                                                                           |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Application boundary       | Retained authority anchor and private namespace container. Intended private children may be nested beneath it; media may not. Not an approved destination.    |
| Private state              | Local SQLite, its WAL/coordination files, and process ownership. Separate from media, cache, and staging.                                                     |
| Cache                      | Private and evictable. Separate from media, staging, state, secrets, and approved evidence. Preserve selected-cache failure behavior without SQLite fallback. |
| Staging                    | Private. Keep its existing OS ownership lock through every staged-file operation. Do not replace a live marker.                                               |
| Media libraries            | Separately configured existing roots. Main/backup role does not grant writes. Browse and scan remain read-only.                                               |
| Other protected references | Configuration, credentials, managed certificate state, browser assets, and stable lock files keep their current exclusions and permissions.                   |

Reject equal, nested, or aliased roots where the current policy requires
separation. Check every admitted and reserved root, including roots currently
offline. An offline root does not become absent or lose its reservation.
Reject new authority when separation cannot be proved. Cached browsing remains
available under the existing observation rules.

Keep SQLite on a verified supported local filesystem. A path spelling or mount
name is not proof of local storage. Do not extend SQLite support to network
filesystems under this work.

Configuration inspection stays passive. The reviewed storage requirement makes
the main JSON file operator-managed and read-only to application components.
Editing it must not bind roots, create directories, migrate SQLite, change
active process handles, or approve media work. The existing JSON save and
API-only reload paths must be replaced under the confirmed policy.
New storage-selection fields would require typed inspection and validation,
generated TypeScript contracts, and the agreed coordinated restart procedure.
First launch may create a safe starter file once; an existing file must never
be rewritten or replaced after a validation failure. No such fields are
introduced by this experiment.

## Candidate binding and its limit

The experiment compares this candidate:

```text
boundary native identity
complete identity chain from that boundary to the configured root
```

It omits only ancestors above the boundary. It retains intermediate directories,
not only the final root identity. The test implementation first verifies that
the chain starts at the actual opened boundary.

Boundary, intermediate, and final-directory replacement must change the
candidate. A copied marker cannot preserve directory identity. Windows must
continue to use full volume/file identity and reject reparse traversal.

This representation solves only equality across different container ancestors.
An ancestry-membership check cannot establish separation across arbitrary bind
mounts. Different final identities do not mean that two roots are disjoint.
Container-local mount IDs are not persistent identity either.

The replacement keeps this equality representation but adds independent,
transient relationship evidence. Neither half grants authority by itself.
Do not treat mount text, an operator path name, a marker, or a successful
equality test as separation proof.

The Linux [mountinfo manual](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)
defines namespace-local mount IDs, their reuse, filesystem device values, and
filesystem-relative mount roots. The [statx manual](https://man7.org/linux/man-pages/man2/statx.2.html)
describes handle-relative metadata. The experiment instead uses
[fdinfo](https://man7.org/linux/man-pages/man5/proc_pid_fdinfo.5.html) to correlate
live descriptor mount IDs and inodes with mountinfo and native handle metadata.
Its required `ino` field dates from Linux 5.14; missing fields fail explicitly.
Docker Desktop also places the Linux daemon in a VM; see
[Docker bind-mount constraints](https://docs.docker.com/engine/storage/bind-mounts/).

Keep mount administration a trusted operator boundary. Define mount-change
checks before new effects. Do not claim protection against native ID reuse,
cloned volumes, or a malicious host administrator.

## Isolated checks

The unit-test-only `filesystem/boundary_prototype.rs` uses the real
`LibraryDirectory` primitives. Normal builds do not include it. Its temporary
database uses separate prototype tables, never the production schema.

Run on Windows:

```powershell
.\dev.ps1 test --lib infrastructure::filesystem
```

Run on Linux:

```sh
cargo task test --lib infrastructure::filesystem
```

The prototype checks boundary-relative equality; changed boundary, intermediate,
and final identities; copied markers; retained-handle reads; invalid relative
paths; and three concurrent fresh processes using one generated SQLite binding.
The process roles are prototype readers, not the production API or worker.
The existing filesystem tests cover symlink/reparse rejection and retained
handles during path replacement.

The ignored `boundary_prototype_child` test is a subprocess entry point. The
concurrent-process test runs it explicitly for all three children. It is not a
skipped safety test.

## Docker counterexample

The PowerShell experiment uses an already installed image. It never pulls an
image, starts the API or worker, accesses providers, or mounts an existing host
directory. It creates a uniquely named volume and uses read-only container
filesystems with networking and capabilities disabled. It removes its exact
generated containers and volume on exit. The generated volume is writable for
setup; the probe calls only `root-identity`.

```powershell
.\scripts\storage-boundary-prototype.ps1
```

Use `-Image` to select an existing compatible image that has
`/opt/domus-obscura/domus root-identity` and `/bin/sh`. That option selects a
program to execute, not trusted evidence about this checkout. The JSON result
records image identity, labels, and its limited evidence scope.

The generated backing volume contains:

```text
volume
  media
  payload
    staging
  media-alias
  staging-alias
```

Each probe mounts the same volume at `/fixture`. Additional mounts expose
`payload` at `/fixture/media-alias` and `payload/staging` at
`/fixture/staging-alias`. No privileged container or host mount command is used.
The paths below are Linux container paths.

The observed candidate chains are:

```text
media:    [boundary, payload identity]
staging:  [boundary, staging identity]
```

Neither chain contains the other root identity. Yet opening
`/fixture/media-alias/staging` returns the exact identity of
`/fixture/staging-alias`. Staging is inside media through the backing filesystem.
This is an actual generated alias, not a string-prefix simulation.

The script requires concurrent containers with different `/` identities. It
checks stable retained identities in both containers, removes one container,
and repeats the check in a new container. It then requires the hidden-alias
counterexample. Exit code zero means the experiment reproduced that failure;
`result` is `rejected_unsafe_candidate`, never design acceptance.

The initial Windows-host Docker run reproduced both equality success and
alias-separation failure. Its existing candidate image reports source revision
`429fbc33c5137131c703a54b8a153766939398f6` with a dirty-source label. This is
exploratory kernel-identity evidence, not current-checkout production integration
or native Linux-host acceptance. Native Linux-host and full API/worker
recreation evidence remain open.

## Explicit migration requirements

No migration is implemented or authorized in this stage.
Migration support is not required for disposable pre-production state.
The remaining requirements in this section apply only if an optional
state-preserving migration is later proposed. Explicit fresh setup must not
reset old data automatically.

Preserve existing immutable root records and their meaning. A later schema
upgrade may add versioned records, but cannot activate a new binding by itself.
Do not silently rebind or change the old equality rule.

The later approved protocol must require all state users to stop. It must
preview the exact old records, observed complete identities, proposed authority,
all root relationships, and affected derived sharing evidence. Confirmation
must bind to this exact preview and expected database state. Recheck filesystem
evidence before one atomic commit. Reject stale confirmation, changed roots,
partial root sets, and conflicting inventory. An interrupted or uncertain commit
needs explicit inspection, not an automatic new approval.

When the old ancestor chain is unavailable, leaf equality is insufficient.
Define and approve an explicit inspection/attestation procedure, or block the
migration. Never accept copied owner markers as substitute evidence.

Preserve immutable plans, approvals, receipts, retained versions, and recovery
state. Sharing saves ancestry and directory hashes in addition to root records;
audit those dependencies. Derived catalogs may need explicit invalidation and
rebuilding. Do not rewrite old evidence or infer sharing approval.

## Review gate and remaining work

Reject the simple boundary-chain design for arbitrary mounted roots. Before any
production implementation, present a revised relationship proof and supported
layout matrix for separate approval.

Native Windows fixtures, Windows-host Linux containers, native Linux execution,
and Linux-host containers are separate evidence. A failed gate or missing host
is not a pass. Full concurrent application behavior, explicit fresh setup, mounted-root
changes during work, and recovery remain required later-stage checks.

Keep Docker networking, missing bind-source rejection, packaging, and release
verification in [#66](https://github.com/PlagueHO/domus-obscura/issues/66).
This experiment does not resolve those gates or authorize host changes,
existing-library writes, or public exposure.

## Replacement proof

### Identity and relationship have separate contracts

The proposed saved binding retains a versioned scheme, the role, the native
boundary identity, and every native component identity from that boundary to
the configured root. The chain must start at the actual retained boundary.
Its final element is the root, not a second fallback identity. A changed
boundary, intermediate directory, or endpoint invalidates the binding.

This is a new scheme for explicit fresh setup, not a reinterpretation of old
rows. An optional state-preserving transition remains separately approved.
The role also carries its ordinary ownership and permission requirements.
Neither filesystem coordinates nor mount IDs become persisted identity or
approval. The experiment's candidate arrays are diagnostics, not database rows.
`null` means a diagnostic root is outside the chosen prototype boundary.

Opening a binding must separately produce a short-lived relationship witness:

```text
retained no-follow directory handles and native identities
  + descriptor mount IDs and matching descriptor inodes
  + matching mountinfo device values and filesystem-root coordinates
  + unchanged mount snapshots and reopened component/descriptor evidence
  -> overlapping | separated | unknown
```

Registration must inspect every admitted/reserved root outside a transaction,
then verify the expected inventory before its short commit. A later operation
must obtain fresh evidence and retain its handles through I/O. Offline or unknown
roots remain reserved. A successful relationship check does not grant writes.

### Restricted Linux algorithm

The test-only policy is in `filesystem/topology_prototype.rs`. Both the normal
Rust test suite and the standalone Linux probe compile this same file.

1. Read bounded `/proc/self/mountinfo`. Open every configured root component with
   `O_DIRECTORY | O_NOFOLLOW` and retain all handles.
2. Read the actual descriptor's `mnt_id`, inode, and descriptor path. Require its
   inode to match handle metadata. Require one matching mount record and matching
   native device major/minor values.
3. Use that mount's filesystem-relative `root` plus the descriptor's suffix below
   the mount point to obtain a backing-filesystem coordinate. This is not a
   comparison of operator path strings.
4. Equal native roots overlap. For roots on the **same ext4 device**, equal or
   nested coordinates also overlap. Component comparison rejects a prefix
   lookalike such as `media-extra`. ASCII case-insensitive comparison makes
   ambiguous case variants conservative, not falsely separate.
5. Different/unsupported filesystems are unknown, not separate. Non-ASCII,
   malformed, unavailable, oversized, or uncorrelated evidence fails explicitly.
   The prototype accepts at most 1 MiB, 4,096 mounts, 16 KiB per mount line, 32
   requested roots, and 256 components per opened path.
6. A mount at or below either compared root blocks a separated result. This
   includes an external private root mounted inside otherwise separate media.
7. Reopen configured paths without following links and compare complete component
   identities. Recheck retained descriptor evidence and require the complete mount
   snapshot to remain unchanged before returning results.
8. A child walk checks every newly opened descriptor against the admitted root's
   mount ID. Reject a crossing before returning that child handle, including a
   bind mount on the same filesystem. Continue to retain the selected root.

The standalone collector uses only the standard library. Its deliberate
`/proc/self/fd/<owned-fd>` magic-link access names a live handle; supplied
components still cannot be symlinks. It never uses a checked native path as the
later data-open path. This collector is experimental tooling, not the future
production adapter. Integration should use existing native handle-relative
primitives and a reviewed Linux metadata adapter, without adding a path bypass.

Mount administration and procfs are trusted. Mount snapshots are not an atomic
filesystem transaction. Their equality cannot exclude a change-and-restore race.
The prototype does not protect against a hostile host administrator, concurrent
uncoordinated directory relocation, cloned filesystems, native ID reuse, or
arbitrary changes after checking. Keep topology stable during authorized work,
retain handles, and reject mount crossings during traversal. A changed witness
requires explicit reinspection, not rebinding or cached separation.

### Generated Docker experiment

```powershell
.\scripts\storage-topology-prototype.ps1
```

The script selects an **already installed** Linux x86_64 Rust SDK image matching
`rust-toolchain.toml`. Use `-CompilerImage` to select an existing matching image
explicitly. Both image metadata and the actual compiler must match the pin.
It never pulls or installs anything. It copies only the two prototype
source files into a uniquely generated volume, compiles them offline with denied
warnings, and runs its tests. It does not execute the old application image.
Containers have no network, no capabilities, and read-only root filesystems.
Only the generated setup/build volume is writable; metadata probes run as
`1000:1000` with read-only mounts. Exact generated resources are removed on exit.

The original `payload` and nested `payload/staging` aliases still defeat the
boundary-only ancestry check. The replacement returns `Overlapping` in both
directions. Ordinary sibling private/media roots and prefix lookalikes return
`Separated`. Equal aliases and direct nesting return `Overlapping`.

An additional fixture mounts the private root at
`/fixture/mounted-media/injected`. The media/private comparison is unknown, not
separate; the child walker explicitly rejects the actual mount crossing.
An ordinary child walk returns the expected native identity.
Missing roots, symlinks, and parent escapes fail with an error.

Two containers coexist with different namespace-root identities. Their retained
binding arrays agree. The script removes one container and checks again in its
replacement. These are isolated metadata processes, not the API/worker lifecycle
or concurrent SQLite acceptance.

The JSON result is `restricted_replacement_passed`, with source hashes, image ID,
compiler/test output, relationships, and limits. It means only this restricted
experiment passed. The final Windows and Linux prototype checks both use the
repository's Rust 1.99.1 pin. The Linux probe is still an isolated build, not a
production application build.

### Measured support and remaining gates

| Environment or case                                                               | Evidence                                                                                                              | Current conclusion                                                                   |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Native Windows generated directories                                              | Existing real handle, replacement, symlink/reparse, marker, and concurrent SQLite tests; new relationship-model tests | Existing protections pass. Linux model tests are not a new Windows topology adapter. |
| Windows-host Linux Docker, generated local ext4 volume                            | Current prototype source, actual bind aliases, child walks, side-by-side containers and recreation                    | Restricted relationship experiment passes. No production API/worker integration.     |
| Missing, changed, malformed, opaque, or different-filesystem evidence             | Model tests; real missing-root and procfs checks                                                                      | Explicit error or unknown. Never authorizes separation.                              |
| New bind mount during a live operation                                            | Real existing child bind mount rejected; changed snapshot model rejected                                              | An actual live mount/unmount race was not induced. Host topology must remain stable. |
| Network media, Docker host-file sharing, non-ASCII coordinates, other filesystems | No replacement admission proof                                                                                        | Requirements remain open. No blanket product ban and no invented support.            |
| Native Linux host, Linux-host Docker, Windows containers                          | No run on these hosts                                                                                                 | Not verified.                                                                        |
| Host/VM reboot, production simultaneous API/worker/CLI use, recovery              | No new evidence                                                                                                       | Later integration acceptance remains required.                                       |

The next review can accept the separation of stable binding identity from
short-lived relationship evidence and this measured Linux subset. It cannot
approve arbitrary layouts or close issue #68. Keep runtime integration blocked
until that separate review and explicit handling of the required deployment
layouts.
