# Shared storage client: contract and relationship experiments

## Status

This is the design and implementation record for
[#68](https://github.com/PlagueHO/domus-obscura/issues/68).
The user approved production implementation under the operator-trusted logical
filesystem contract after reviewing the prototypes. Production integration is
implemented. Issue #68 and its attached acceptance evidence record the exact
source, platform checks and remaining delivery gates for each revision.

The user selected one private application directory with overridable child
folders and separate primary/backup media libraries. Media can be local or on
network file shares; SQLite must remain local. See the
[application storage layout](storage-layout.md) for the seven confirmed policy
decisions, including default paths, read-only host configuration, coordinated
restart, and explicit fresh setup. The current production model is in
[initial production selection](#initial-production-selection). Progress and
prototype sections below retain their original evidence and limits.

**Confirmed trust boundary:** The user selected
"Use operator-trusted filesystem bindings (Recommended)" after rejecting
physical-device properties as a storage prerequisite. The
[current authority contract](#confirmed-hardware-independent-authority-contract)
supersedes earlier requirements for physical backing graphs, Intel member
mapping, globally clone-resistant identity and independently proved separation
across opaque providers. Those investigations remain historical evidence,
not production prerequisites. The production adapter now retains ancestor handles
and queries actual filesystem locality. New selected records save logical root
meaning. Existing native records are never converted or adopted.
Host JSON writes and API-only activation have now been removed.

**The simple retained-boundary proposal failed its alias-separation gate.**
Removing container-specific ancestors makes identity comparison stable, but does
not prove that media and private storage are separate. Do not use the experiment
to authorize storage. That historical experiment changed no production binding
semantics, configuration fields or database migrations. The later approved
production integration is recorded below.

**Restricted replacements now reject real Linux and Windows aliases.** Linux
correlates retained directory handles with kernel mount-root coordinates.
Generated same-ext4 fixtures pass sibling separation, alias rejection,
descendant-mount refusal, and identity comparison across containers.
The native Windows experiment uses retained handles and normalized volume GUID
coordinates. It covers local same-volume NTFS siblings, drive/GUID aliases,
junction refusal, and replacement. Neither experiment approves runtime
integration or arbitrary mounts. Network media, different filesystems, native
Linux hosts, and Windows containers still need separate evidence.
See [replacement proof](#replacement-proof).

Selected production access refuses old native root records. It never converts
or adopts them. Bare native access remains available to isolated fixtures.
Container concurrency and recreation now have a passing generated-data proof;
native Linux-host evidence remains separate.

## Confirmed hardware-independent authority contract

Storage authority concerns filesystem objects and directory relationships, not
the disks that hold their bytes. Separate filesystems may share physical media.
Do not require disk serials, controller identifiers, RAID membership or a physical
backing graph. This design does not establish independent backup failure domains.
A filesystem behind RAID is not excluded merely because controller membership
metadata is unavailable. The observed Intel-backed Windows filesystem remains
in scope through its supported logical filesystem interface.

The operator controls configuration, mounts and share mappings. Treat that
administration as trusted. Stop all components before changing those mappings,
then validate and restart them with the same frozen selection. Do not claim
protection against a privileged administrator making undisclosed topology changes.

Keep three different responsibilities:

1. **Logical selection:** operator-owned configuration selects the provider,
   root and role. A stable application root ID identifies a record; it is not
   proof that a directory is the approved one.
2. **Live filesystem authority:** open components without following links.
   Retain handles, verify the expected native identities within the supported
   operator-selected provider scope, and reject unexpected replacements.
   A copied marker cannot replace this check. Filesystem IDs are not physical
   device IDs, and must not be assumed portable across clients or containers.
3. **Relationships:** reject known equal, nested or aliased roots where policy
   requires separation. Use actual namespace evidence where the supported
   provider supplies it. Do not infer independence from different labels,
   paths, UUIDs, hosts or signing keys.

For opaque mappings between provider views, an explicit operator topology
assurance can supply the missing relationship assumption. Bind that assurance
to the exact selected roots and configuration/binding revisions, not a blanket
"trust all storage" flag. It is trusted administrative input, not a measured
separation proof. Without a matching assurance or supported relationship evidence,
keep the relationship unknown and refuse new admission. An assurance cannot
override a known overlap, unsafe link or changed native binding.

The operator must ensure that private roots are not also exposed as media through
another provider. The application cannot promise automatic discovery of hidden
aliases across opaque systems. A deployment needing independent verification
must supply a supported trusted namespace bridge or remain refused; that is not
the default requirement for every deployment.

An exact copy of the filesystem identifiers and trusted configuration may be
indistinguishable from the original using those same inputs. That case needs
explicit operator handling or an authority outside the copy. Automatic detection
of all such clones is not a guarantee of this model. Ordinary detectable root
replacement, unsafe traversal and stale-binding refusal remain required.

The user subsequently approved production implementation under this contract.
The document below distinguishes historical test-model schemas from implemented
production behavior. Current supported profiles, exact assurances, fresh setup
and stopped-host restart rules are implemented. No automatic rebinding is
available. Complete final platform validation before claiming acceptance.
Keep SQLite local, configuration read-only, component traversal handle-relative,
and media effects subject to their existing approval and recovery rules.

## Production adapter progress

`LibraryDirectory` now keeps all opened ancestor handles. Its canonical form
returns the directory whose actual handles supplied the verified native chain.
Capability-style child opens and clones retain that chain. The old
`into_directory` escape is removed. Cache, staging, publication, recovery,
certificate, startup, configuration and sharing consumers keep the shared
directory object instead of discarding its ancestor handles. Metadata and
permission queries can borrow a cloned file handle while that object stays live.
SQL remains in `Store`.

On Windows, the retained handles prevent ancestor rename until their owner ends.
On Unix, a rename can succeed; retained handles still reference the original
objects. These are different native behaviors, not a portable pathname lock.
The new regressions exercise child opens and clones after the initial root ends.

The locality guard now inspects the opened filesystem, not just its path prefix.
Windows queries reopen the exact borrowed object synchronously through
`NtOpenFile` with an empty relative name. They query
[`NtQueryVolumeInformationFile`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntqueryvolumeinformationfile)
and, for nonremote objects, `GetVolumeInformationByHandleW`.
This avoids `ReOpenFile`, which failed on generated directory handles in the
native Windows check. Only nonremote disk filesystems reporting `NTFS` receive
the current local classification.

Linux uses `fstatfs` on the actual file or directory handle. The current local
classification covers the ext filesystem family. Its shared magic value does
not distinguish ext2, ext3 and ext4 or establish a durable provider binding.
Recognized NFS and CIFS/SMB filesystem types report remote; other types report
unknown. State and secret guards reject remote and unknown classifications.
No path spelling, physical-device query or owner marker substitutes for this
check.

Passive path validation does not open inactive references or require new parents
to exist. Active reads inspect both the opened parent and the opened file.
First-run directory creation checks the actual existing or created parent.
This keeps startup validation separate from filesystem effects and catches a
file mount whose filesystem differs from its parent.

Configuration loads and passive draft validation now resolve nonempty relative
native paths from the configuration file's parent. Absolute overrides stay
unchanged. Empty paths and Windows rooted paths without a drive remain invalid.
The saved JSON and its raw-byte revision are not rewritten during resolution.

The production filesystem module now supplies `DirectoryExpectation` and a
retained directory guard. Its versioned codecs are
`windows-file-id128-chain-v1` and `linux-exportfs-chain-v1`. An expectation covers
the selected provider boundary and every relative directory below it. Reopening
checks each actual handle before descending. Ancestors above that boundary stay
retained but are not part of this expectation. No bare directory capability is
returned through the public guard.

The Linux native adapter queries the borrowed object with
[`name_to_handle_at` and `AT_EMPTY_PATH`](https://man7.org/linux/man-pages/man2/name_to_handle_at.2.html).
It pairs the opaque handle with the opened filesystem's identifier. It never uses
`open_by_handle_at`, a mount ID, a physical-device identifier, an inode-only
fallback or the object's former path. The adapter accepts at most 128 opaque
handle bytes. Unsupported filesystems, larger handles and syscall restrictions
return errors. This is an adapter bound, not a guarantee about all filesystems.

Generated Windows and WSL ext4 fixtures check serialized-chain reopening and
refusal of replaced boundaries, intermediate directories and leaves despite
copied markers. Linux also checks link refusal and the borrowed native handle
after path replacement. Provider admission, scope persistence, locality and
relationship assurances remain separate. These primitives do not change saved
production bindings or establish container acceptance.

The default first-launch path now uses `config.json` and `state/state.db` under
the agreed personal OS root. `serve --storage-root` selects an explicit absolute
root without adding another application folder. The saved database reference is
relative to the configuration file, not the working directory.
First launch refuses a nonempty application directory when
its main configuration is absent. It does so before creating its startup lock
or pending configuration and before opening SQLite. Existing databases,
orphaned sidecars, copied locks, unknown files and media are not adopted or
removed. A concurrent setup rechecks under a lock opened through the retained
directory. Existing invalid configuration fails without creating that lock.
This refusal guard is not the complete configurable setup or external
state-binding implementation. Omitted database and enabled optional-role paths
now use `state/state.db`, `cache`, `staging` and `certificates` relative to the
configuration file. Omitted optional services remain disabled.
The native Store integration now uses a fixed private `sqlite-tmp` child.
Its full integration checks remain incomplete.

The retained-consumer, directory-profile and fresh-refusal version passed full
`check`, `build` and `smoke` on Windows and Ubuntu 24.04 WSL. Its main library
suite passed 414 Windows tests and 408 WSL tests, with 15 ignored on each.
Workspace integrations, the separate harness and 266 frontend tests passed.
An earlier default-parallel WSL run failed the scoped-refresh worker's completion
assertion. That exact test passed alone; the full two-thread rerun passed.
The failed log is preserved. These counts precede the host-configuration change
below. WSL used a verified generated source copy on ext4. It does not establish
native Linux-host or concurrent-container acceptance.

Existing host JSON now has no application writer or API-only reload lifecycle.
The old save and reload routes return `410 Gone` under both SQLite policy flag
values. Browser guards remain active. System settings shows redacted saved and
active summaries with coordinated restart guidance, while versioned SQLite
output policy and artist-photo bindings retain their existing commands.
`domus configuration-check` validates saved JSON without opening a listener.
It reports invalid input through redacted issues and a failing exit status.
Managed renewal and validated manual TLS material refresh remain separate.
The host-configuration snapshot passed Windows `check` and Ubuntu 24.04 WSL
`check`, `build` and `smoke`. Desktop and narrow browser cases passed with the
installed Edge executable after pinned Playwright Chromium was unavailable.
No browser was installed. These checks precede the latest layout and selection
changes; they do not validate the final integrated storage implementation.

### Initial production selection

The new `filesystem/selection.rs` module defines a frozen `FilesystemClient`.
Supplied host `storage` configuration selects versioned roots, provider mappings
and exact topology assurances. Each root has a role, stable record ID, binding
revision, provider scope, relative `LibraryPath` and complete native directory
expectation. Provider boundaries resolve from the configuration file.
Raw source and canonical semantic revisions remain separate.

`LogicalRootBinding` serializes one versioned complete root definition without
its runtime provider locator or ancestors above the selected boundary.
Its stable digest is a record identifier, not directory or write authority.
`FilesystemClient::open_binding` requires the exact current root definition and
the requested role before it verifies the complete native chain. A new runtime
ancestor chain can therefore reopen unchanged saved meaning. A changed binding
revision, role or boundary cannot reuse it.

The existing SQL root registry now saves these records under
`storage-root:<root_id>`. Native paths remain runtime locators, not database keys.
Selected production access refuses native records. Bare isolated fixtures refuse
logical records. Neither mode converts or adopts the other representation.
Filesystem admission precedes SQL transactions; registration inside a transaction
only compares immutable definitions and writes SQL.

Scans, availability, indexed JPEG reads, publication and recovery reopen registered
roots through the current mapping. Cache owner records and staging receipts use
the stable binding identity. Staging keeps its current native locator separately
for playback path reporting. Sharing bindings and catalog directory evidence save
logical descendant snapshots: the complete root definition, a root-relative path
and every persistent component identity from the selected root. The snapshots
contain no handles and cannot authorize access without a matching fresh traversal.

Admitted directories and their descendants retain the selected client. Its raw
root pins retain current native ancestry without creating an ownership cycle.
Known runtime overlap fails even when an assurance says that roots are separate.
A live client refuses a changed runtime ancestor chain. Release all owners before
an intentional locator change and restart with the unchanged logical selection.

Managed certificate journals, pending recovery and history archives also save
logical directory evidence. Certificate status, startup, ownership and renewal
use admitted certificate directories. Explicit bootstrap can initialize only an
already selected, empty certificate root. Unknown files, existing locks and copied
journals are not fresh state. Runtime certificate and credential locators do not
change the frozen hostname, authority, resource and credential-content binding.
There is no conversion for a native-bound journal.

Service configuration and credential inputs must be inside an admitted private
boundary and outside known media ancestry. Installed browser assets can remain
outside application storage, but cannot enter known media roots. This exception
does not apply to secrets or writable application state.

Input reads admit the actual parent directory before opening its child file.
They retain that handle through the bounded read. Worker requests, source
listings, local artwork, service requests, credentials and manual TLS refresh use
this path. Credential checks do not need a SQL transaction. Browser loading uses
the same bounded reader and no-follow directory iteration, with the separate
non-media rule for installed assets.

Selected service protection also validates the bounded SQL root inventory
against the frozen selection. Native or stale saved root records remain an
error, even when the input directory itself has a valid private role.
This check does not reopen offline media or convert old root records.

### Inspecting an additional root

Stop all state users before changing an operator selection. Create any required
empty private directory yourself with the required permissions. Inspect its
complete expectation without changing configuration:

```powershell
domus storage-root-inspect 'G:\' media-home main 1 media Music
```

The command prints one `StorageRoot` candidate. Its arguments select the absolute
provider boundary, provider scope, root ID, positive binding revision, role and
library-relative path. An empty relative path selects the boundary itself.
The directory must already exist. Inspection creates no files, registers no root
and gives no media-write approval.

Review the candidate and the complete layout. Add its provider mapping and root
to the operator-owned `storage` configuration. Add exact topology assurances
where required. Advance the selection revision for a changed selection. Keep the
existing external state expectation unchanged. Then run `configuration-check`
and restart all components with the same configuration. A candidate is not
proof that the combined layout is admissible. Do not replace an existing saved
root definition to make incompatible state pass.

The production semantic digest uses `domus-storage-selection-v2\0`. It covers
the selection version and revision, sorted provider scope IDs, complete
sorted root records and the expected coordination-file identity. Provider boundary locators are excluded. Relative provider
resolution and container-local mount paths must not invalidate an otherwise
unchanged logical selection or its exact assurances. Each root's complete native
expectation still verifies its selected boundary and every relative component.
A locator that opens a different boundary is refused. The raw source digest
still binds the exact host configuration bytes. This production digest is
separate from the historical test-model digest below.

The `application` role protects the containing private directory from media.
It can contain the other private roles. The reverse containment is refused.
An opaque application/private relationship still needs an exact assurance.
State, cache, staging, certificate and
media roots keep their separate relationship checks. A supported complete chain
within one unchanged provider/filesystem scope supplies namespace evidence.
A filesystem transition or opaque mapping requires an exact operator assurance.
Known native overlaps remain refused even with an assurance. Offline selected
roots remain reserved without being opened during another root's admission.
An individual root must pass its own native check before access.

Fresh setup saves application/state expectations outside SQLite before database
initialization. `ConfigurationService` requires those selections and retains both
source-directory and selected-root authority. API startup, passive draft database
reads, certificate commands, the production worker, service CLI and MCP host
loading use that selected authority when opening Store. Service CLI and MCP use
the main host schema, not a second unselected service-host schema.
An absent selection fails before their startup lock or SQLite effects. Copying
configuration to another application root does not authorize that root.
Configured locators are compared through complete retained native ancestry.
Ordinary and extended-length Windows path spellings can identify the same
selection; another directory cannot.

State utility commands now require `--host`. Worker/check, publication and source
metadata commands also require the selected main host. Their operation files
must identify that same database through its native file identity. They cannot
select a bare database or migrate old state. Publication remains independent of
acquisition-provider enablement. Source listing import reopens through the
retained selected source after local input validation.

Explicit `init --storage-root` creates fresh private configuration and state
without browser validation, provider activation or a listener. Existing
configuration and nonempty storage are refused without changing their evidence.
Shared-policy and root-binding commands check the host's read-only policy.
The development setup script uses the generated application configuration and
keeps its generated library outside the application directory.

This is not complete production acceptance. The selected Windows regressions now
cover logical media, sharing, cache and staging reopening after provider relocation.
Certificate journal and archive relocation also passes without rewriting saved
bytes. These generated checks are not concurrent-container or native Linux-host
proof. Complete the remaining consumer audit and final platform checks before
claiming acceptance.

### Selected state coordination

Fresh setup creates the private `.domus-storage-selection.lock` file before it
captures external expectations and opens SQLite. Host storage configuration
records its full persistent identity in `expected_selection_file`. Selected
state access refuses a missing or replaced file, including a copied valid record.
It does not recreate or adopt that file. Database names cannot use this reserved
name, including ASCII case variants.

The fixed record contains a version header, a 64-character digest and a SHA-256
checksum. The checksum detects damaged records; it is not authentication.
The fence digest covers the semantic selection digest and all exact topology
assurances in canonical order. Assurances reference the semantic digest, not
the fence digest, so there is no circular reference. Provider locators and raw
host bytes are not part of this cross-process token. Different local mount paths
can share an unchanged logical selection. Each client still retains its exact
raw source revision and verifies actual native boundaries before access.

Matching selections hold shared OS locks. A different selection needs exclusive
ownership before it can update the existing coordination record. Admission
rechecks the record after converting back to shared ownership. This record update
is also possible during read-only database admission when no conflicting owner
exists. It does not write host JSON, create SQLite state, bind a root, change a
saved plan or grant an approval. Passive JSON parsing and selection inspection
do not acquire this fence. Stop all state users before operator configuration
changes; the fence does not replace that procedure.

The native adapter owns the original locked file handle. It keeps that handle
through independent connections, retained filesystem clients, operation locks,
deferred statements, blobs, backups and quarantined close failures. It checks
the file entry, native identity and exact record before SQLite file effects.
Rust owner destruction must not unlock an earlier duplicate. The terminal native
release closes the fence only after dependent files and directory pins.
Malformed, truncated and oversized records fail without repair.

### Native SQLite integration in progress

The five Store open modes now use the pinned native SQLite namespace adapter.
The small [build wrapper](../crates/native-sqlite-sys/README.md) downloads and
verifies the exact upstream archive, then applies the native namespace patch
inside ignored build storage. Complete upstream SQLite and SQLCipher sources
are not committed. The patched engine must match the accepted engine byte for
byte. First build needs HTTPS; a verified cached archive supports
`CARGO_NET_OFFLINE=true`. Package notices retain the MIT license, SQLite notice,
source pin and patch. This changes dependency packaging, not runtime authority.

The adapter retains actual no-follow state ancestry and the private `sqlite-tmp`
directory through SQLite file operations and deferred native lifetimes. Main,
sidecar and temporary names do not fall back to ambient pathname opens.
Read/current modes refuse a missing temporary directory without creating or
repairing it. Only explicit create mode can create that private child.

`StoreSource` creates independent connections from admitted native authority.
It retains the Rust directory chain and the complete frozen filesystem client,
including offline root reservations and the source revision. Independent owners
do not reconstruct that selection from database rows or reported paths.
Worker supervisors and lanes, acquisition, publication recovery, local runtime,
sharing catalogs and visibility, account reconnects, response spools, CAA and
Commons use the retained source. State locks and overlap checks use its admitted
directory, not a new database-parent lookup. WAL, FULL synchronization, foreign keys, the
five-second busy timeout and independent read snapshots remain Store policy.
Terminal close failures retain native evidence and produce an error report.

Five focused Windows Store cases passed. They cover open modes and snapshots,
real private temporary spill, refusal of missing temporary storage or a replaced
state root, source-owned state locks and the complete frozen selection across
independent thread owners while media is offline.
Windows retains state and TEMP pins that prevent directory rename. Unix can
permit rename, but subsequent source opens and lock effects must fail without
touching either a replacement root or the detached original.

The selected-startup and Windows locator regression passed 90 cases with two
ignored. A later complete check passed 418 library cases with 15 ignored before
the frozen-selection addition. It exposed old cache and catalog service-host
fixtures. The corrected cache and CLI selection regressions passed 42 cases.
The later Windows check passed the integration targets and local harness, then
found missing selection in the browser-workflow fixture. After that correction,
all seven browser-workflow cases passed. The host-selected CLI stage also passed
28 worker/bootstrap cases, all 33 CLI cases and the publication CLI case with
acquisition providers disabled. Complete checks for this later source are pending.
The adapter alone also
passed six Windows and six pinned Ubuntu WSL cases. None of these results is
native Linux-host or concurrent-container production acceptance.

These historical check results precede the logical registry and certificate
integration described above. They do not complete the shared storage client.
External selection and cross-process native fencing now protect selected Store
entry and dependent ownership. Current complete checks and the required native
Linux-host and concurrent-container evidence remain open. Locality validation
alone is not sufficient.

## Concurrent container regression

The ignored `logical_store_container_probe` is a private generated-data test.
Build the Linux library test executable with the pinned tools. Use the executable
reported by `cargo test --locked --package domus --lib --no-run`.
Run the controller with a compatible runtime image that is already cached:

```text
node scripts/storage-container-proof.mjs <Linux-test-executable> <cached-runtime-image> <new-evidence-directory>
```

The controller requires a Linux Docker engine. It does not pull or build an
image. It creates one new named volume and four bounded containers with network
access disabled. Each container has a different runtime ancestor chain.
An independent reader keeps its one-row snapshot while the writer commits
20 additional rows. Both complete before their containers are removed.
A new container then reopens and scans the unchanged retained storage.

The controller compares the immutable root bodies, retained media identity and
byte-for-byte saved selection. It records the executable hash and runtime image
ID. Label-checked cleanup removes only the generated containers and volume.
Any test or cleanup failure produces a failing exit status. Preserve the result
directory as evidence. Docker Desktop execution is Windows-host evidence,
not native Ubuntu-host evidence.

The first current production-client run passed all four modes. Its executable
SHA-256 was
`739341c5cf7f8e8edd9245f7dc316bb1de97cf214a7105ae7a583f635c28db07`.
The saved selection SHA-256 stayed
`433b0a8d537dca41f558cf9a3c5ab5aa1649acd337e96787ebb66fb800b3c3f1`.
Runtime ancestry differed in each mode. The recreated reader observed all
21 rows and unchanged generated media. All generated resources were removed.
This result predates the later cache error-order correction. Final validation
must identify its exact source separately.

## Proposed logical binding document and admission model

The test-only
[`logical_binding_prototype.rs`](../crates/domus/src/infrastructure/filesystem/logical_binding_prototype.rs)
models the configuration checks for this trust boundary. It does not change
production configuration, startup, native identity encoding or database access.
The format below is a review proposal, not an accepted production schema.

### Document fields

| Field                                     | Proposed meaning                                                                                                                                                 |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `format_version`                          | Version of the binding document. The model accepts version 1 only.                                                                                               |
| `selection_revision`                      | Nonzero operator revision for the selected storage arrangement.                                                                                                  |
| `roots`                                   | Complete selected root inventory with unique `root_id` values and exactly one state root.                                                                        |
| `root_id`, `binding_revision`             | Stable record ID and nonzero revision for one binding. Neither is native directory authority.                                                                    |
| `role`                                    | `state`, `cache`, `staging` or `media`. These are separated roots, not the containing application directory or a complete model of protected private references. |
| `provider_scope`, `locator`               | Operator-selected logical provider view and root location. Labels and path spelling do not establish identity or separation.                                     |
| `expected.profile`, `expected.components` | Supported provider-owned identity format and ordered directory expectations. A real adapter must verify these against retained no-follow handles.                |
| `topology_assurances`                     | Exact canonical root pairs, each with both binding revisions and the semantic selection digest. No global assurance is accepted.                                 |

Unknown fields, unsupported versions/profiles, zero revisions, duplicate roots,
invalid state inventories and malformed or stale assurances are refused.
Assurance pairs use increasing root-ID order. Self-pairs, reversed pairs,
duplicate pairs and references to missing roots are refused.

The diagnostic bounds input to 65,536 bytes, 16 roots and 64 identity components
per root. Root/provider/profile names are limited to 128 bytes, locators to 4,096
bytes and each identity component to 512 bytes. These are test-model bounds,
not approved product limits or a promise to accept arbitrary native path forms.

The portable model uses the synthetic `fixture-handles-v1` identity profile.
The Windows-only extension below adds an actual handle-based profile.
There is no Linux or SMB identity codec in this model.
Provider profiles still need complete production validation, continuity scope
and reconnect behavior. Preserve full Windows file IDs.
Do not make transient Linux `st_dev` values, mount labels or arbitrary provider
names durable identity authority.

### Selection and activation

The **selection digest** uses SHA-256 with the domain prefix
`domus-storage-selection-v1\0`. It covers the format version, selection revision
and complete bindings sorted by root ID. This includes each role, locator,
provider scope, expected identity and binding revision. It excludes assurances
to avoid a circular hash. JSON whitespace and inventory order do not affect it.
A changed mapping invalidates an assurance even without a revision increase.
The digest binds content; it is not a signature or proof of operator identity.

The separate **source digest** covers the exact document bytes, including
assurances and whitespace. The model refuses an active source digest or
observation digest that differs from its frozen selection. Production must
bind this check to the complete host configuration and coordinated process
lifecycle. The model takes a supplied active digest; it does not implement a
configuration reader, cross-process agreement or restart protocol.

### Admission and SQLite ordering

Admission requires a complete current observation batch for the frozen selection.
Each observation must match its binding revision, provider scope and expected
directory chain, and report retained no-follow traversal. State must report
verified local storage; remote and unknown classifications are refused.
Every required root pair needs current relationship evidence:

| Relationship                          | Admission result                                                                       |
| ------------------------------------- | -------------------------------------------------------------------------------------- |
| Measured separation                   | Admit with an observed basis.                                                          |
| Opaque provider mapping               | Admit only with an exact current operator assurance; record an operator-assured basis. |
| Known overlap or unsupported topology | Refuse, even with an assurance.                                                        |
| Missing evidence                      | Refuse.                                                                                |

The result is a policy review, not a serializable filesystem capability.
The portable model observations are synthetic, including traversal and filesystem
classification. They do not prove native handle safety or storage locality.
The ordering test uses fresh temporary directories and real generated SQLite
databases. Invalid model observations must fail and leave the directory empty;
valid observations must succeed and permit writing and reading the exact value.
This tests the proposed order, not production bootstrap or the native SQLite
adapter.

### Proposed setup and rebinding lifecycle

1. **Existing state:** Load operator-owned configuration without SQLite effects.
   Reject missing, unsupported or mismatched expectations before database
   creation, coordination, recovery or migration. Do not recover expectations
   from SQLite, an owner marker or a new scan of the selected path.
2. **Explicit fresh setup:** Select a new empty state location without deleting
   old state. Inspect the roots and present exact bindings and any assurances
   for operator review. The operator installs the reviewed configuration.
   Database creation follows successful live admission, not passive inspection.
   Safe starter-file creation remains the separate create-once exception.
3. **Explicit rebinding:** Stop all components. Review old and proposed
   selections against current root evidence. Change binding/selection revisions
   and replace obsolete assurances explicitly. Do not silently bless a
   replacement, migrate data, discard receipts or move media.
4. **Restart:** Validate the complete configuration and restart all components
   with the same exact selection. Do not use an API-only reload to activate it.
   New work must use retained authority and current versions throughout its
   operation. Existing recovery and submitted-effect receipts remain protected.

These lifecycle steps are design requirements, not implemented commands.
Native profiles, live observation collection, private-reference coverage,
production SQLite bootstrap and coordinated activation remain separate work.
Production implementation is now approved. These lifecycle commands and
production enforcement remain incomplete.

**Earlier model-only result:** All six model tests, four native Windows profile tests and
`.\dev.ps1 check` pass on Windows.
The full check includes contracts, formatting, Clippy, Rust tests, browser
harness tests and frontend checks. This result is not native provider acceptance
or evidence of production integration. This check predates the production
adapter changes above.

### Windows native directory-chain extension

The test-only
[`logical_binding_native_windows.rs`](../crates/domus/src/infrastructure/filesystem/logical_binding_native_windows.rs)
adds `windows-file-id128-chain-v1`. It uses the existing no-follow directory
operations and borrowed-file high-resolution identity query. It retains the
selected boundary and every directory component through the requested leaf.
Its ordered expectations contain the boundary identity followed by each child
identity, excluding ancestors above the selected boundary.

Each component uses `win128:<16 hexadecimal digits>:<32 hexadecimal digits>`:
the 64-bit filesystem volume serial number and full 128-bit file ID.
These are logical filesystem identifiers, not hardware serials or RAID members.
There is no short-ID fallback. Microsoft documents their
[same-computer file identity scope](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info).
This extension does not establish portability across Windows hosts, containers
or SMB clients, or detection of indistinguishable copied filesystems.

The generated-fixture tests reopen serialized expectations after releasing the
first handles and read exact sentinel bytes through the retained leaf handle.
Separate cases replace the boundary, intermediate directory and leaf. Every
replacement must fail even when its owner-marker bytes match.
Live ancestry detects known equal/nested roots, including a child opened from
a different selected boundary. Non-overlap remains opaque; it is not promoted
to measured separation.

Actual native identity does not establish SQLite locality. The extension reports
`Unknown` filesystem classification, which must refuse state admission before
any SQLite creation. It does not substitute a path prefix or filesystem label
for handle-based locality evidence. Locality, complete relationship evidence,
restart/remount continuity and production bootstrap remain open.

## Client contract

Extend the existing filesystem adapter rather than adding a storage service.
Use one concrete Rust client per process. Add a trait only at a demonstrated
I/O or testing boundary.

| Responsibility                                                                                                       | Owner                                    |
| -------------------------------------------------------------------------------------------------------------------- | ---------------------------------------- |
| Open each component without following links; retain handles; collect native identities; establish root relationships | Shared filesystem client                 |
| Save and query root records, versions, observations, and receipts                                                    | Existing SQLite `Store`                  |
| Coordinate inspection, fresh setup, registration, claims, approvals, and effects                                     | Application use cases                    |
| Select configured roots and construct the client                                                                     | API, worker, and CLI composition         |
| Normalize audio, decode images, and enforce provider rules                                                           | Existing adapters and application policy |

The client must not own a SQLite connection, execute SQL, approve a media
destination, acquire a job claim, or contact catalog/acquisition providers.
An approved network-filesystem adapter can perform storage transport I/O.
Consumers must not reimplement its filesystem checks.

Keep these operations distinct:

1. **Inspect:** return bounded evidence or a typed error without changing state.
2. **Open an existing binding:** verify saved authority and return retained
   handles for the requested operation.
3. **Register:** inspect existing directories and save an explicitly admitted
   root. Do not create or move media.
4. **Fresh setup:** create new state only through explicit setup or the authorized
   first-launch starter path. Never convert or rebind incompatible old state.

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
Reject new authority when neither supported relationship evidence nor the exact
operator topology assurance establishes the required relationship under the
confirmed contract. Known overlaps remain refused. Cached browsing remains
available under the existing observation rules.

Keep SQLite on a verified supported local filesystem. A path spelling or mount
name is not proof of local storage. Do not extend SQLite support to network
filesystems under this work.

Configuration inspection stays passive. The reviewed storage requirement makes
the main JSON file operator-managed and read-only to application components.
Editing it must not bind roots, create directories, migrate SQLite, change
active process handles, or approve media work. Host JSON save and API-only
reload paths have been removed under the confirmed policy.
New storage-selection fields would require typed inspection and validation,
generated TypeScript contracts, and the agreed coordinated restart procedure.
First launch may create a safe starter file once; an existing file must never
be rewritten or replaced after a validation failure. No such fields are
introduced by this experiment.

## Production binding-design review

The user selected this review after the authenticated diagnostic passed.
**The client boundary is defined below, but runtime integration is not ready.**
The diagnostic does not supply general network operations or a common authority
for client-local and remote roots. Different-volume local admission also needs
evidence. These gaps affect the intended separate primary/backup disks and
network-share development layout. Keep both requirements; do not hardcode the
operator's example drive paths.

### Current implementation gaps

| Current mechanism                              | Verified limitation                                                                                                          | Required replacement                                                                                                                                                 |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `filesystem/identity.rs::NativeRoot::overlaps` | Saved ancestor membership returns a Boolean. `false` does not establish separation across hidden aliases.                    | Explicit overlapping, separated or unknown results from current, handle-correlated evidence.                                                                         |
| `filesystem.rs::LibraryDirectory::open`        | Intermediate handles build ancestry, but only the final directory remains in the returned object.                            | Keep the relevant ancestor/root handles alive with the operation authority.                                                                                          |
| `filesystem.rs::require_local_disk`            | Path-prefix checks do not identify the actual filesystem. A drive letter or ordinary Unix path can hide network storage.     | Inspect the actual opened state-directory/filesystem handle and reject unverified SQLite backing.                                                                    |
| `store/native_roots.rs::register`              | Saved serialized ancestry is used for new separation checks. The record has no current provider/topology proof.              | Collect evidence outside SQL, then commit against an unchanged complete root inventory.                                                                              |
| `store/private_roots.rs`                       | Reservations cover staging/cache and reject path nesting. They do not model all agreed private child roles.                  | Protect state, secrets, immutable approvals/receipts, configuration and other private references. Allow only intended containment beneath the application container. |
| `store.rs` database opens                      | SQLite receives a pathname after local-path/canonicalization checks. A directory authority is not passed to the SQLite open. | Define and prove state-file and journal/WAL/shared-memory opens against the protected local state root.                                                              |
| Configuration consumers                        | Host JSON writers and API-only activation are removed. Processes still load independent snapshots.                           | Use one resolved startup selection and refuse conflicting active revisions before new effects.                                                                       |

This is a source review, not a reproduced exploit or a runtime fix.
The existing binding commands and database remain unchanged.

### Minimal production boundary

Extend `infrastructure::filesystem`. Keep one concrete shared client per
process, with native and any approved network I/O adapters at real boundaries.
Do not add a storage daemon, SQLite client abstraction or independent copy of
relationship policy.

The client owns live handles, bounded native observations, filesystem
classification and safe child operations. `Store` owns immutable bindings,
inventory versions and durable observations. Application use cases own
registration, claims, approvals and effects. The state-root bootstrap expectation
is a specific exception: it is operator-owned startup information in `config.json`,
outside the database it authorizes. API, worker and CLI compose these
parts from the same startup selection.

Keep three values distinct:

1. **Configured locator:** an operator path or supported network-root reference.
   It selects a candidate, not authority. Relative private children stay beneath
   the application root; outside overrides must be explicit absolute paths.
2. **Saved binding:** role, binding format/version, full native identity within
   the supported operator-selected provider scope, and applicable topology
   assurance revisions. Preserve full Windows IDs. Do not use a marker,
   namespace-local mount ID or transient signing context as durable identity.
3. **Live authority:** retained root/component handles, current binding and
   inventory revisions, filesystem classification, complete required
   relationships and operation-specific permissions. Never serialize handles
   or treat a saved observation as current I/O permission.

Provider labels name the operator's selected scope; they do not prove independent
storage or namespace separation. Different signing keys, server GUIDs, device
values or filesystem UUIDs alone do not prove a root relationship. A remote
witness cannot attest a client-local private root that it cannot observe.
Use the exact operator topology assurance for opaque mappings under the confirmed
trust contract, not an invented physical-backing proof.

Use one pure Rust implementation of relationship policy. Native evidence
collection and SMB transport stay in infrastructure. The experimental C module
must not become a second implementation of the business rule.

### Registration and operation protocol

Read the complete relevant binding inventory and its revision. Open candidates
without following links. Retain their handles. Collect current identity,
filesystem class and all required relationship pairs outside SQL transactions.
Missing or stale relationships refuse new registration. Opaque pairs require
the exact operator topology assurance selected by the confirmed contract.

Commit an immutable binding in a short transaction only if the inventory and
configuration revisions still match. A concurrent registration must make the
commit fail. Never collect filesystem or network evidence inside that transaction.
Do not overwrite an existing binding or silently upgrade its meaning.

Open existing bindings separately from registration. Recheck the selected role,
native identity and current topology before new effects. Use the retained parent
for each child open. Reject alternate streams, root escapes, links/reparse points,
unexpected mount traversal and stale evidence. Keep staging ownership locks alive.
Check private-file permissions on the actual no-follow handle.
A changed binding ends the live authority; restoration requires fresh authority.

An offline root retains its reservation. It does not become absent or separated.
Cached index browsing can remain available under existing observation rules.
Do not add new roots or effects based on unavailable separation evidence.
Keep receipts for already-submitted work and preserve uncertain submission state.

The application container can contain its intended private child roles. It is
not a media destination. State, evictable cache, staging, secrets and immutable
approved evidence remain separate where their policies require it. Protect
configured external private references too. Do not introduce a blanket exemption
for overlap between private roots.

### SQLite and configuration

SQLite stays in `Store` on verified local storage. Its main file, WAL, shared
memory, journals and coordination files must remain in the protected state root.
Do not assume that returning a checked pathname establishes this guarantee.

[SQLite's open interface](https://www.sqlite.org/c3ref/open.html) accepts a filename
and optional VFS selection. It documents `SQLITE_OPEN_NOFOLLOW`, but that option
alone does not establish the required retained-parent and sidecar behavior.
The [VFS interface](https://www.sqlite.org/vfs.html) owns OS file operations and
locking. Any needed adapter must preserve compatible locking for API, worker and
CLI. This review does not select or approve a custom VFS. Prove the smallest
correct approach before changing database opens.

#### Isolated SQLite state-root proof

The approved experiment is available through:

```powershell
.\scripts\storage-sqlite-prototype.ps1
```

Use `-Platform Windows` or `-Platform Linux` to run one target. The Linux target
is a Linux container on the Windows development host, not a native Linux-host
deployment. `-CompilerImage` selects an already installed SDK; the script never
pulls an image or installs tools. `-EvidencePath` saves the JSON result outside
the generated fixture. It verifies the installed SQLite source archive against
`Cargo.lock`, then extracts only the engine and header. No vendored SQLite source
or application dependency is added.

The measured source is SQLite 3.53.2 from `libsqlite3-sys` 0.38.2. Both standalone
C probes compile against that source. GCC and MSVC compile the probe code with
warnings denied. The Linux fixture has no network, host mounts, ports or
capabilities. Its root filesystem is read-only; memory and swap total 512 MiB,
with one CPU and 128 processes. Tests run as `1000:1000` in newly created private
directories. Native Windows tests use the installed MSVC environment without
elevation, new disks, accounts, ACL changes or service changes.

The baseline uses ordinary SQLite WAL behavior. After opening the main file,
the generated test moves its parent, creates a replacement parent and adds a
hard-linked name for the original database in that replacement. This preserves
the main-file identity and avoids SQLite's moved-file refusal. Actual opened
WAL and shared-memory identities then match the replacement directory, not the
retained original. This is an observed counterexample, not a claim that every
ordinary directory rename bypasses SQLite's moved-file checks.

The Linux candidate retains the state directory and its parent. Its test VFS
uses an internal owned-descriptor name, while the optional Unix `open` hook
actually opens fixed main/sidecar names through `openat` on that retained
directory. It rejects leaf links and non-regular files. Metadata and deletion
use `fstatat` without link following and `unlinkat`. A new connection checks the
original parent/name against the retained identity. A mismatch permanently
invalidates that context, including after name restoration. Existing SQLite
mechanical I/O stays in the retained directory. A restored root requires a fresh
test authority; the old context is not silently rebound.

The Windows candidate opens every directory component separately through
`NtCreateFile` and retains all handles without delete sharing. It requires actual
local NTFS/device evidence. The optional Windows `CreateFileW` hook opens SQLite
main/sidecar files relative to the retained directory through `NtCreateFile`,
with no reparse traversal. VFS access and deletion use those native handles.
An actual rename returns access denied while the pins are open. It succeeds
after every pin closes, and a replacement directory has a different full
128-bit identity. Reparse targets are refused without changing their bytes or
removing the generated junction.

Both candidates keep the built-in VFS file methods, locks and shared-memory
mapping. Two independent processes each commit 50 rows. Queries require every
expected primary key, not only a row-count proxy. Linux workers also finish
against the original root after parent replacement; the replacement stays empty.
Separate rollback-journal tests exercise actual native journal opens and normal
cleanup. WAL remains the primary acceptance mode, not a workaround replaced by
rollback journaling.

Interrupted writers commit a row, flush a later uncommitted write and exit
without connection cleanup. Fresh processes preserve committed rows, exclude
the uncommitted value and perform normal WAL cleanup. This is process-interruption
evidence, not power-loss, reboot or storage-controller durability proof.
Linux additionally refuses actual root, main-file, WAL, shared-memory and journal
symlinks and a FIFO without blocking or changing the refused input.

The later production-contract review reproduced one additional failure in the
original Linux diagnostic. Its permission check opened and closed the main
database outside SQLite. While a rollback transaction held a reserved lock,
that close released the process's POSIX locks and let an independent process
start another writer. The original concurrent-commit checks did not exercise
this permission-query case.

The corrected diagnostic uses native `faccessat2` with the retained directory,
effective credentials and no link following. It does not open another database
descriptor. An unavailable syscall returns an I/O error, not a pathname or
glibc-emulation fallback. A new regression holds the reserved lock, performs the
actual permission query and requires the other process to receive `SQLITE_BUSY`.
It fails with the old code and passes with the correction.
This permission query is only a VFS access result; the later native open still
enforces no-follow, regular-file and handle ownership requirements.
See [SQLite's POSIX close warning](https://www.sqlite.org/howtocorrupt.html#_posix_advisory_locks_canceled_by_a_separate_thread_doing_close_)
and [native faccessat2 semantics](https://man7.org/linux/man-pages/man2/access.2.html).

The results do **not** select a production adapter.
[SQLite documents syscall overrides as optional testing facilities](https://www.sqlite.org/c3ref/vfs.html).
The original diagnostic assumes one active root per process, fixed database/sidecar names,
trusted procfs on Linux and cooperative topology administration. Temporary
databases, spill files, attached databases, multiple independent roots, general
Unicode/case behavior, remounts, cloned backing, other filesystems and hostile
administrators are not admitted by this experiment. SQL remains in `Store`;
these C probes are not another domain-policy implementation.

The original source-matched evidence is `files/storage-sqlite-root-proof.json`
in the session. Its source hashes belong to that historical run. The later
temporary-file proof below records the current source separately.
The harness removes its exact generated containers, volume, junction,
databases and build/source directory. Manual development fixtures are separately
removed and checked. No production Rust, database opens, configuration consumers,
commit, push or PR changes follow from this proof. A production I/O contract
without diagnostic hooks and the separate integration approval remain required.

#### Proposed production database I/O contract

The user directed continuation of this contract review. The following requirements
do not select a source fork, approve runtime integration or extend the measured
prototype support.

`Store` keeps the connection, SQL, schema checks, migrations and transactions.
The shared filesystem client supplies one non-serializable state authority,
including retained ancestors, actual local-filesystem evidence, current binding
and the frozen startup revision. Database I/O uses that authority, not a new
path-based filesystem implementation. Provider services and workers must receive
the same selection instead of converting `Store::database_path()` back into
independent connection or process-lock authority.

Preserve the four existing open behaviors in `store.rs`: explicit creation or
upgrade, existing mutable open, current-schema mutable open without migration,
and current-schema read-only open. Snapshot reads still own their bounded SQL
transaction. Preserve the five-second busy timeout, foreign keys, WAL and full
synchronization. Read-only SQL can still require shared-memory coordination;
do not present it as permission for arbitrary filesystem writes or hide this
behavior by setting `immutable=1`.

State-root admission must precede any creating, recovering or coordinating open.
Existing expected bindings must not be inferred from a copied database or owner
marker. Initial setup and existing-state admission need an explicit bootstrap
contract; reading SQLite to find its own authority must not create sidecars or
migrate an unverified replacement. This bootstrap contract remains an integration
prerequisite, not an implemented shortcut.

**Confirmed bootstrap location:** The user selected `config.json` for the
operator-approved expected native state-root binding. Do not add another
application-managed binding file or write an existing operator configuration.
Native handles, transient mount coordinates and live permission observations
do not belong in this saved expectation.

Startup must read bounded, no-follow operator configuration, resolve its selected
state candidate and retain native ancestors. It must verify the expected native
identity within the supported operator-selected provider scope before any SQLite open can
create files, coordinate, recover or migrate. Missing, malformed, unsupported
or mismatched expectations fail explicitly without falling back to current
filesystem identity or a copied database/owner marker.
Only after admission can `Store` perform its requested open behavior.
Recheck the live authority before new effects and retain it through driver cleanup.

First setup and state relocation remain explicit operations. Fresh initialization
must publish a create-once configuration with its approved expectation before
initializing SQLite. Failure or an existing config must not cause replacement,
silent rebinding, automatic state deletion or partial success.
The precise schema and initialization protocol are not implemented by this
location decision. A later explicit operator binding update requires the same
coordinated stop/edit/validate/restart as other host changes.

The current prototypes do not yet supply the serialized identity contract.
Unix `st_dev`, retained descriptor labels and transient witness contexts must
not become durable provider identity. Define and verify logical provider/root
continuity within the operator-trusted mapping before accepting persistent state
bindings. Physical backing discovery and automatic refusal of indistinguishable
full clones are not prerequisites under the confirmed contract.
Media registration still requires supported namespace relationships or exact
operator topology assurances. Keep unsupported providers refused.

Cover every SQLite file class used by the current engine, not only the four
fixed diagnostic filenames. Main files, WAL, shared memory and rollback journals
stay paired in the state root. TEMP databases, statement journals, sorting and
query spill files, and maintenance scratch need explicit private-state authority.
The user selected the fixed private `sqlite-tmp` child beneath the selected state
directory, with no independent configuration setting or OS temporary-directory
fallback. A state-directory override moves this child with state.
The layout decision does not approve production integration. Do not force all work into memory
or disable journaling to avoid temporary-file support.
Attached external databases and `VACUUM INTO` destinations need separately admitted
authorities if a supported command requires them; this review does not add those
commands. Do not let URI options or unknown file classes bypass selected roots.
See [SQLite temporary-file classes](https://www.sqlite.org/tempfiles.html).

Keep one SQLite engine copy per process and preserve compatible locking across
connections and processes. On Unix, ordinary filesystem-client opens, duplicates
and closes of active database or coordination files must not bypass the driver's
lock/descriptor coordination. Metadata checks should use retained-directory
operations or borrow the already managed handle without closing it.
Application process locks remain separate stable files; they do not replace
SQLite database or WAL locks. Never retain a connection across `fork`.

The SQLite file object must retain its state authority through close, shared-memory
unmap and deferred descriptor cleanup. Failed opens must not leave dangling
callbacks or close another connection's managed descriptor. A busy close must
not release handles that still own statements or shared memory. Stale authority
blocks new work while preserving required cleanup and recovery files. Do not
automatically remove journals, WAL or uncertain temporary outputs after failure.

The adapter must satisfy the documented
[VFS file-method contract](https://www.sqlite.org/c3ref/io_methods.html):
zero-fill short reads, distinguish contention from I/O failure, honor synchronization,
preserve lock upgrades/downgrades and WAL mapping/barriers, and report only actual
device guarantees. Return `SQLITE_NOTFOUND` for unsupported file controls.
Unknown capabilities must not become claimed atomic-write or durability support.
Mechanical SQLite I/O is distinct from external provider/library I/O; the latter
still stays outside SQL transactions.

The user selected the thin pinned source adapter for the next isolated proof.
The alternatives and their trade-offs remain:

| Route                      | Main benefit                                                                                                                                          | Main cost                                                                                                                                              |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Thin pinned source adapter | Retains upstream file methods, locking, WAL mapping and descriptor coordination while connecting namespace operations to the shared native authority. | Requires a maintained, provenance-preserving source patch and a reviewed native callback ABI. SQLite upgrades must re-audit every relevant operation.  |
| Complete public VFS        | Uses SQLite's public VFS API without a source fork.                                                                                                   | Must implement and prove the full native file, locking, shared-memory, temporary-file and failure contracts rather than copying the diagnostic facade. |

The selected investigation preserves mature SQLite locking and recovery code.
It does not approve a production fork or claim that the complete route is safe.
Optional syscall hooks, private-struct
casts, dynamic symbol interposition and default-VFS fallback are not production
interfaces. Keep the existing borrowed-file ID patch separate and unchanged.

Before integration, the selected route needs actual same-process and independent
process contention, metadata/permission checks during locked transactions,
reader/writer/checkpoint overlap, TEMP and spill work, error injection, failed
open/close lifetimes, stale roots, recovery and consistent maintenance tests.
Require exact committed values and refused-input preservation. Native Linux-host,
Windows-host Linux-container and Windows-native results remain separate.

#### Production connection ownership: independent owners selected

The bounded owner-thread proof is not the selected production threading model.
It proves one active native context and a capacity-one request channel.
It does not prove that native callbacks can safely share a root across concurrent
connection owners.

The current application already has independent connection owners.
`CaaArtworkService::fetch` in `infrastructure/services.rs` opens connections
inside separate `spawn_blocking` tasks. Its provider orchestration keeps a
mutable `Store` across asynchronous provider work on that blocking thread.
`PreparedProductionWorker` separately opens current-schema connections.
A single SQL owner thread would therefore require changes to the current
orchestration, not just replacement of the SQLite open function.

| Candidate                                                              | Effect on current callers                                                                                                                              | Further proof required                                                                                                     |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| Independent connections sharing the admitted state authority, selected | Keep the current blocking-task connection pattern. Each connection and its dependent objects have one owner at a time.                                 | Thread-safe shared native authority, concurrent callbacks, cross-connection deferred closes, quarantine and final cleanup. |
| One Store-owned SQL thread per process                                 | Send database work through one bounded queue. Refactor callers that retain a Store across provider work so network waits do not occupy the SQL thread. | Queue admission, cancellation, shutdown, cross-process contention and preservation of existing transaction boundaries.     |

This choice must not put SQL or a connection inside the filesystem client.
SQLite's [threading modes](https://www.sqlite.org/threadsafe.html) do not prove
the safety of application callbacks or permit unsynchronized access to one
connection. Do not add an unsafe `Send` or `Sync` implementation to make the
isolated `Rc` owner shareable.

Preserve all five existing Store entry points through the admitted authority:

| Existing entry point           | Creation  | Migration | Read behavior                                    |
| ------------------------------ | --------- | --------- | ------------------------------------------------ |
| `Store::open`                  | Allowed   | Allowed   | Mutable connection.                              |
| `Store::open_existing`         | Forbidden | Allowed   | Mutable connection.                              |
| `Store::open_current_existing` | Forbidden | Forbidden | Mutable current-schema connection.               |
| `Store::open_read_current`     | Forbidden | Forbidden | Read-only, without a long-lived SQL transaction. |
| `Store::open_read_snapshot`    | Forbidden | Forbidden | Read-only snapshot with its own schema check.    |

The next isolated proof must use actual connections to the same admitted
generated root. Check exact committed rows during reader/writer overlap, retain
authority through deferred native closes, and reject a different startup
selection before new SQLite effects. An actual native close failure must
quarantine admission without invalidating existing cleanup owners.
Do not treat reference counts, a successful open, or serialized test execution
as proof of concurrent native ownership.

`Store::database_path()` currently supplies reopen paths to providers, workers
and process locks. Those callers must receive the admitted authority instead.
Sharing code also obtains `sharing_connection()` outside Store; its SQL must
remain in Store when that escape is removed, not move into the filesystem client.
These are caller-review findings, not runtime changes.

The user selected "Keep independent connection owners (Recommended)".
This preserves current connection concurrency. It authorizes the isolated
ownership proof, not production integration. Provider/root identity, bootstrap
and the separate production integration gate remain required under the confirmed
hardware-independent authority contract. Intel member mapping is not a gate.

##### Bounded independent-owner result

Run `1ea7ea90774e4f4b88d7187f2e2ac687` passed on native Windows/NTFS and in the
existing capability-free Linux/ext4 container as UID 1000. Both use the pinned
SQLite source bridge and Rust 1.99.1. No Azure resource or controller command ran.

The new Rust fixture shares an actual directory `File` through `Arc`.
Connections and statements remain thread-confined. It adds no unsafe `Send` or
`Sync` implementation and leaves the earlier `Rc` fixture unchanged.
Two different owner threads meet inside SQL function callbacks, so the proof
cannot pass through one SQL owner thread or a mutex around all SQLite calls.
Fixture metadata and native open/close callbacks use an
[application mutex](https://www.sqlite.org/c3ref/c_mutex_fast.html), never held
across an upstream SQLite call. Temporary-file method tables now belong to each
file instead of a shared mutable array.

The reader retains an 8,192-row snapshot while the writer commits another 256
rows. Every snapshot value and all 8,448 current values match exactly.
Both owners write real private TEMP databases. The retired reader connection
retains its native authority through deferred statement cleanup and checks all
8,192 temporary rows with their complete 1,032-byte payloads.
This follows SQLite's documented
[`sqlite3_close_v2` lifetime](https://www.sqlite.org/c3ref/close.html), not an
assumption that a successful close means immediate native release.

A different generated startup selection is refused before any new native open.
Restoring the old selection does not revive that invalidated authority.
The replacement directory remains empty. This is an in-memory selection fixture,
not the operator configuration or durable bootstrap implementation.

One exact TEMP file then suffers an actual native close failure while a peer
connection and its prepared read remain alive. The fixture records one failed
native close call on Linux and three failed calls for that same protected handle
on Windows. New admission is refused. The peer's value `41`, statement cleanup
and native authority remain valid. Final release stays quarantined until the
existing explicit fixture recovery closes the exact failed descriptor or handle.
Both Rust-owned directory pins are independently checked closed.
Each platform ultimately releases two native contexts.

The runner limits each ownership workload to 120 seconds. It removes its fresh
container, volume and scratch directory. The source archive, patch, source files,
headers and runner hashes are recorded in
`files/storage-sqlite-independent-owners-proof.json` in the session workspace.
The earlier Rust ownership workload and both ordinary adapter modes also passed
on Windows and in the Linux container.

This proves one shared diagnostic context, not a production callback registry,
all connection entry modes, cross-process shared-authority ownership or a
production admission protocol. The new independent-owner workload has not run
on native Ubuntu. The earlier native Ubuntu result covers its recorded source
and thread-confined workload only. New native execution needs fresh approval;
the earlier Azure deadline has expired.

#### Measured compile-time source bridge

```powershell
.\scripts\storage-sqlite-prototype.ps1 -Adapter PinnedSource
```

This is the user-selected next isolated proof, not runtime integration.
The harness verifies the cached archive against `Cargo.lock`, checks and applies
`storage-sqlite-source-adapter.patch` only to its generated SQLite source copy,
and compiles that copy with `DOMUS_SQLITE_SOURCE_ADAPTER`.
The installed source/archive and repository dependency manifests stay unchanged.

The patch adds a declared native-I/O version-one interface.
Unix `osOpen` calls the compiled native open function directly.
Windows `osCreateFileW` calls the compiled native create function directly.
The source-mode probes do not call `xSetSystemCall` or `xGetSystemCall`.
Other SQLite file methods, descriptor coordination, locking and WAL mappings
remain upstream code. Existing VFS metadata, deletion and full-name facades
still supply the diagnostic's retained-root namespace.

The first source-mode cases passed with 108 Linux parent assertions and 132 Windows
parent assertions. These counts exclude checks of the removed runtime-hook API;
they do not remove the behavioral cases. Both targets verify all 100 concurrent
worker keys, sidecar placement, interrupted recovery and refused-input preservation.
The corrected Linux permission query still blocks the independent writer with
`SQLITE_BUSY`. Parent-process native bridge counters are positive on both targets.
Child-process counters are separate and are not reported as a combined I/O total.

Historical first-bridge evidence is `files/storage-sqlite-source-adapter-proof.json`.
It records the original archive/engine/header, source patch, native interface
header, patched engine, probes, harness and actual bridge-call counts.
The exact generated fixtures and extracted/patched source are removed afterward.
SQLite's public-domain source provenance remains in the original copied engine;
no complete vendor source is added to this repository. The separate borrowed-file
identity patch remains untouched.

This first source bridge keeps the diagnostic's one-active-root model and fixed
main/sidecar names. It does not prove a thread-safe root registry, Rust callback
ownership, temporary/spill support, bootstrap trust or every auxiliary native
operation. A baseline-only unbound native-open path exists in this test program;
it is not an approved production fallback. Keep those requirements in the
production I/O contract and obtain the separate supported-contract integration
approval before changing `Store` or the runtime dependency.

#### Measured private TEMP and sorting files

The user selected `state/sqlite-tmp` for the next isolated proof. This is a fixed
child of the selected state directory, not another configurable root.
Production `Store`, dependencies and startup configuration remain unchanged.

The current harness extends both source and runtime-hook modes. A VFS request for
an unnamed temporary file generates an exclusive `etilqs_` name beneath retained
temporary authority. It does not set `sqlite3_temp_directory` or call the OS
temporary-directory search. Unknown names/classes and unavailable authority fail;
there is no default-VFS or OS-temp fallback for these requests.
The diagnostic still uses one active root per process.

Linux retains the temporary directory separately. Actual `openat` creation uses
that handle, no-follow, exclusive creation and returned regular-file metadata.
The source patch now routes upstream `osUnlink` through native `unlinkat` for
supported main/sidecar and temporary names. The runtime-hook diagnostic installs
and restores the corresponding testing hook. Upstream shared-memory close also
uses `osUnlink`; refusing all non-temporary names incorrectly left SHM files behind.
The corrected dispatch preserves normal SHM cleanup and the original link tests.
Neither the native permission query nor temporary tracing opens and closes an
extra active database descriptor.

Windows retains the temporary child through parent-relative `NtCreateFile`
without delete sharing. Exclusive file creation preserves the upstream
temporary/hidden attributes and delete-on-close behavior. Native delete-on-close
needs `DELETE` access and `FILE_DELETE_ON_CLOSE`; these are explicit in the
diagnostic. The normal main/WAL/SHM/journal path remains unchanged.

`storage-sqlite-temp-workload.h` shares one SQL workload and a public
`sqlite3_file` forwarding wrapper between targets. The wrapper retains the
generated double-terminated filename until close and counts successful upstream
`xWrite` bytes separately for TEMP databases and other temporary files. It
forwards the upstream file methods, locks and device guarantees, not private
SQLite file structures.
See the [public file-method contract](https://www.sqlite.org/c3ref/io_methods.html)
and [temporary-file classes](https://www.sqlite.org/tempfiles.html).

The workload sets `temp_store=FILE` and an eight-page TEMP cache. It inserts
8,192 distinct keys with exact 1,032-byte payloads, then forces sorting without
an index. Every returned key and complete payload must match the expected order.
Actual native creation plus successful file writes prove disk-backed work;
row count, query plan and memory-only results are not substitutes.
An additional successful spill-write increase is required for each accepted sort.

| Private-TEMP stage mode | Linux parent assertions | Windows parent assertions |
| ----------------------- | ----------------------- | ------------------------- |
| PinnedSource            | 152                     | 153                       |
| RuntimeHooks            | 245                     | 154                       |

Both modes record the same parent-process write totals for each target.
Linux records 22,835,200 TEMP-database bytes and 25,558,701 spill bytes over
initial work, retry and fresh-authority cases. Windows records 11,411,456
TEMP-database bytes and 17,039,134 spill bytes over initial work and retry.
These are cumulative successful writes, not file sizes, unique data or a
memory-use measurement. All five Linux and three Windows successful parent
temporary opens close through SQLite. Interrupted child counters are separate.

The Linux cases refuse an actual temporary-file symlink without changing its
target or removing the link. A replaced temporary directory permanently blocks
new sorting files, including after namespace restoration. A fresh authority
can admit the restored original and produce the same exact output.
A temporary-root symlink also refuses actual TEMP writes.
The Windows cases deny temporary-root replacement while pins are live, refuse
an existing temporary-name directory, and preserve an actual temporary-root
junction and its target. A refused new file can be retried without losing
committed main-database data or valid TEMP contents.

Interrupted writers perform actual TEMP writes and sorting, keep the TEMP
database open, flush an uncommitted main WAL write and exit without SQLite close.
Both targets verify that ephemeral temporary files are absent after process
exit. WAL recovery still preserves committed rows and excludes the uncommitted
value. Normal close removes temporary files and releases the Windows directory
pin. This does not prove power-loss behavior or authorize deleting unknown
leftovers. The original concurrent-key, lock, sidecar and refusal cases still pass.

Private-TEMP stage source evidence is `files/storage-sqlite-private-temp-proof.json`,
run `0400c6b91910434294a12618be96ec42`.
Private-TEMP stage runtime-hook evidence is `files/storage-sqlite-private-temp-hooks-proof.json`,
run `11ce4a213c7344cab55ea538beb73bdf`.
Both record the shared workload header, native header, probes, harness and,
where applicable, checked patch and copied-engine hashes.
Generated sources, databases, containers, volume and junctions are removed.
No installed sources, existing disks, accounts, ACLs, services or media change.

**Limits:** This proves the measured TEMP database and external-sort classes,
not every statement journal, transient index, materialization, maintenance file,
`ATTACH` or `VACUUM` path. Temporary naming, forwarding tables and active roots
are diagnostic, not a thread-safe Rust/native registry. Windows fixtures inherit
host permissions; production handle-based privacy checks remain required.
Linux runs inside the Windows-host Docker Linux VM, not native Linux-host
acceptance. Bootstrap trust, production callback ownership, complete auxiliary
I/O, backing/provider scope and separate runtime approval remain open.

#### SQLite statement lifetime and contention stage

The next isolated proof extends the same native candidates, pinned SQLite engine
and fresh-fixture harness. It does not change production Rust, dependencies or
configuration. Both adapter modes pass the earlier TEMP, concurrent-key, sidecar,
interrupted recovery and link/refusal cases.

A same-process connection opened with `SQLITE_OPEN_READONLY` refuses an actual
SQL insert. It starts an explicit WAL read transaction while another connection
commits one exact new key. The reader retains the old count and value sum;
the writer sees the new state. With zero busy timeout, a TRUNCATE checkpoint
returns `SQLITE_BUSY` and leaves uncheckpointed frames while the snapshot is live.
After the reader commits, truncation succeeds with zero log and checkpoint counts.
A new read sees the exact committed key. Removing only that generated key restores
the original row count and sum. Closing one connection leaves the other usable.
Use database name `main` for defined output counters, as required by the
[checkpoint API](https://www.sqlite.org/c3ref/wal_checkpoint_v2.html).
SQL read-only access does not mean that WAL coordination needs no filesystem I/O.

Both targets keep a prepared TEMP query alive over an actual open temporary file.
`sqlite3_close` returns `SQLITE_BUSY`, leaves the file-close count unchanged and
allows the statement to return the exact sum of all 8,192 keys.
`sqlite3_close_v2` returns `SQLITE_OK` but does not close that file yet.
The retained statement still reads the exact TEMP sum; its finalization then
closes every remaining temporary file. The wrapper separately records upstream
`xClose` errors and requires zero. Callback counts alone would not prove successful
close when SQLite's deferred teardown does not return that error to the caller.
No database API uses the connection after
`close_v2`. Only the existing statement is used. See the
[connection close contract](https://www.sqlite.org/c3ref/close.html).

The Linux reserved-lock case also opens, reads and closes a SQLite-managed sibling
connection after the parent starts a rollback `BEGIN IMMEDIATE` transaction.
The real native permission query still runs. A separately forked contender with
zero busy timeout cannot acquire that transaction: it must receive `SQLITE_BUSY`.
This checks upstream same-process descriptor coordination as well as the corrected
permission callback. It does not open or close an extra database descriptor
outside SQLite.

| Lifetime stage mode | Linux parent assertions | Windows parent assertions |
| ------------------- | ----------------------- | ------------------------- |
| PinnedSource        | 220                     | 220                       |
| RuntimeHooks        | 319                     | 221                       |

Statement-lifetime source evidence is `files/storage-sqlite-lifetime-final-source-proof.json`,
run `f3b1d4a3bdc5437ca97fa5c57ddc9078`.
Statement-lifetime hook evidence is `files/storage-sqlite-lifetime-final-hooks-proof.json`,
run `936b3efd1a8a477cac1f66aaac44ea69`.
Each records one completed read-only snapshot, busy-close and deferred-close case
per target, with zero upstream TEMP-close errors. TEMP/spill write totals and successful temporary open/close totals
remain the same as the private-TEMP stage. Every expected concurrent key remains
present. Interrupted Windows recovery still returns exactly 103 committed rows.
Current source hashes match both records. Exact generated scratch directories,
containers and volumes are independently absent.
The earlier read/write-reader run, intermediate read-only-reader runs and
private-TEMP runs remain historical records with their own hashes.

**Limits:** Root and temporary authority are still held manually by the test
caller through final statement cleanup. These results prove SQLite's delayed
file-close behavior in this candidate, not a production Rust owner, callback
registry, automatic root lease or concurrent root registration. Live BLOB and
backup objects, every auxiliary file class, failed native close, power loss and
native Linux-host acceptance remain unproved. The production adapter, persistent
bootstrap/backing authority and separate integration approval remain blocked.

#### Auxiliary files and dependent cleanup

The next isolated stage exercises the same approved native harness without
production changes. It adds actual disk subjournals, DISTINCT materialization,
VACUUM scratch, short reads, failed-open cleanup and incremental BLOB/backup
lifetimes. The earlier stage records retain their original source hashes.

The TEMP forwarding layer borrows the existing `sqlite3_file` through public
`SQLITE_FCNTL_FILE_POINTER`. A read across the physical end of that file must
return `SQLITE_IOERR_SHORT_READ`, preserve the available two bytes and zero-fill
the six missing bytes. Unknown file control 1000 must return `SQLITE_NOTFOUND`.
No extra database descriptor is opened, duplicated or closed for these checks.

A forced VFS error after successful native allocation initially leaked its
file: the pinned engine's `sqlite3OsOpenMalloc` frees a failed-open object without
calling `xClose`, and `sqlite3OsOpen` expects null methods on failure.
The facade now closes its allocation and clears its public methods before
returning the injected error. The actual sort must report `SQLITE_IOERR`;
every newly allocated native file must close with zero close errors.
The next sort must return every original key and full payload.
See the [public file-method contract](https://www.sqlite.org/c3ref/io_methods.html).
This is an injected facade failure, not every possible native-open failure.

The diagnostic adds an 8,192-row main-database fixture with the same exact
payload oracle. It changes all rows within an outer transaction, changes them
again after a savepoint, and rolls back both levels. This forces a disk subjournal
under both existing WAL and DELETE journal modes. TEMP's normal memory journal
and a small savepoint were not sufficient evidence; no journaling policy was
disabled or changed to hide that result. Every restored payload is checked.

An actual unindexed DISTINCT query writes a transient native database and returns
the exact expected total payload length. VACUUM must allocate and write private
native scratch for the large main fixture. It must preserve every fixture key
and payload. Removing only the generated maintenance table leaves the original
committed value count and sum unchanged. These are diagnostic SQL operations,
not new application maintenance commands.

Incremental BLOB and unfinished source-backup objects each independently prevent
`sqlite3_close`, with the exact TEMP contents still usable afterward.
The deferred-close case holds a statement, a BLOB and a partially stepped backup.
After `close_v2`, finalizing the statement cannot close their shared TEMP file.
Closing the BLOB still cannot close it while the backup owns the source.
Completing and finishing the last backup then closes all remaining TEMP files.
Both memory copies are checked against every key and all 1,032 payload bytes.
No source connection API is called after `close_v2`. No destination connection
API is called between successful backup initialization and its matching finish.
The backup destination is memory using the selected VFS, not another disk root.
See the [BLOB contract](https://www.sqlite.org/c3ref/blob_open.html) and
[backup contract](https://www.sqlite.org/c3ref/backup_finish.html).

| Auxiliary stage mode | Linux parent assertions | Windows parent assertions |
| -------------------- | ----------------------- | ------------------------- |
| PinnedSource         | 278                     | 317                       |
| RuntimeHooks         | 377                     | 318                       |

Final source run is `b26243e58c3d45b0b7122a1aba55b64b`, in
`files/storage-sqlite-auxiliary-source-proof.json`. Final hook run is
`17a4f5cb31b54b78b7a9d8591c9886e4`, in
`files/storage-sqlite-auxiliary-hooks-proof.json`.
Linux measures 22,394,200 subjournal bytes and 40,144,896 transient-database
bytes; Windows measures 11,197,100 and 40,144,896 respectively.
These are cumulative successful writes, not file sizes or memory measurements.
All ten Linux and eight Windows parent temporary files close with zero errors.
All earlier concurrent-key, snapshot, lock, link/refusal and interrupted recovery
cases still pass. At that stage, source hashes matched both final records.
Exact generated scratch, containers and volumes are independently absent.

**Limits:** The caller still manually retains one root through cleanup. A
thread-safe Rust authority and actual callback/root ownership remain unproved.
Backup to an independently admitted disk root, native-close failures, URI/file
control namespace escapes, other maintenance classes, power loss and native
Linux-host acceptance remain separate requirements. These results do not admit
general filesystem providers or authorize production integration.

#### Isolated SQLite namespace refusals

The next isolated fixture adds six negative cases: an unexpected filename,
parent traversal, a `file:` URI with an unexpected filename, alternate VFS
selection through a URI, an external `ATTACH` destination and an external
`VACUUM INTO` destination. These do not add application commands.
[SQLite documents](https://www.sqlite.org/uri.html) that URI parameters can select
a VFS and that URI support also applies to attached databases.

The first Linux pinned-source run reproduced a diagnostic fault: rejecting an
unexpected filename also marked the retained root invalid. A later valid open
then failed. The corrected `xFullPathname` rejects bad input without changing the
root's validity. An actual changed or unreadable root still invalidates it.
The existing replacement and stale-root refusal cases continue to pass.

Valid reopen and the live connection retain every exact ordered committed key
after the negative cases. The alternate-VFS test also requires the actual native
guard's refusal counter to advance, so a read-only container filesystem or an
unrelated permissions error cannot stand in for the required guard. Failed
`ATTACH` and `VACUUM INTO` leave no added database attached.

| Namespace stage mode | Linux parent assertions | Windows parent assertions |
| -------------------- | ----------------------- | ------------------------- |
| PinnedSource         | 320                     | 889                       |
| RuntimeHooks         | 419                     | 890                       |

These counts include per-key assertions, not a coverage or performance measure.
Each target passes all six cases in both modes. The source run is
`2a15ca054a894f02aea0130341e41a15`, in
`files/storage-sqlite-namespace-exact-source-proof.json`. The hook run is
`d3b0c92ca26949bdbcac5dfdc092a6a4`, in
`files/storage-sqlite-namespace-exact-hooks-proof.json`.
Earlier namespace runs remain evidence for their earlier source revisions.
All earlier auxiliary, exact-payload, contention, snapshot, link/refusal,
temporary cleanup and interrupted recovery cases pass. No installed SQLite
source, dependency or runtime Rust consumer changed.

**Limits:** These six cases do not prove all URI options, file controls or
maintenance commands. Memory databases and URI options such as `immutable` or
`nolock` still require an explicit production entry-point policy. Thread-safe
Rust callback/root ownership, native-close failures, independent bootstrap
authority and native Linux-host acceptance remain open. Linux results here use
Windows-host Linux containers. Production integration remains unapproved.

#### Isolated Rust ownership and bounded owner thread

The initial Windows ownership proof uses the pinned-source SQLite build and the
existing native fixture. Rust owns the actual native context. Its final drop
unregisters the VFS and closes the retained directory handles only after all
temporary-file callbacks finish. It does not merely count Rust references.

Run the isolated proof with installed tools and fresh generated state:

```powershell
.\scripts\storage-sqlite-prototype.ps1 -Platform Windows -Adapter PinnedSource -RustOwnership
```

In three separate cases, the application root owner drops and
`sqlite3_close_v2` retires the connection before statement, BLOB or backup work
finishes. Each dependent wrapper retains the native context through its last
public SQLite cleanup call. This follows SQLite's
[deferred-close contract](https://www.sqlite.org/c3ref/close.html): a successful
`close_v2` does not mean that its native resources are already destroyed.
The proof never reuses a retired connection pointer.

The statement and backup cases each verify all 8,192 ordered keys and every byte
of each 1,032-byte payload. The BLOB case verifies its complete 1,032 bytes.
An actual directory rename fails while each context is retained. It succeeds
after the last dependent cleanup returns, with no remaining temporary files.

A fourth context runs on a separate owner thread. Two client threads submit
requests through a channel with capacity one. Only the owner thread touches
SQLite or the native context. Each request verifies another 8,192 complete rows.
`Rc` prevents moving the native lease to a client thread; no unsafe `Send` or
`Sync` implementation is added. Both clients finish before channel shutdown,
connection retirement and native handle release.

The run records four native context releases, 16,384 rows in the owner-thread
stage, source hashes and removal of generated resources. Source-matched run
`930f5659d2494df8a48bb57609f032de` is recorded in
`files/storage-sqlite-rust-owner-thread-final-proof.json`. The existing Windows
fixture also passes its 889 parent assertions and recovery/refusal cases in run
`5bdd1f01f0e04bc58e660dd2d146fba1`. This removes a false
dependency on Intel member-layout research: this ownership proof needs no
controller metadata or additional host permissions.

**Limits:** This is one active native context at a time, fixed filenames and
thread-confined native callbacks. It proves a bounded caller-to-owner-thread
route, not a general concurrent callback registry or the final application
threading design. This initial stage did not include native-close failure,
URI/file-control policy or Linux ownership. The following sections record those
later proofs. Startup authority and native Linux-host acceptance remain open.
Production Rust, dependencies and consumers remain unchanged. Production
integration still requires the separate supported-contract review.

#### Native close failures and Linux Rust ownership

The same Rust statement, BLOB, backup and bounded owner-thread cases now run on
both native Windows and the existing isolated Linux/ext4 container. Linux keeps
actual parent, root and temporary-directory descriptors until the last lease
finishes. Native metadata checks prove that they remain open. After cleanup,
`fcntl(F_GETFD)` confirms that each descriptor is closed. Linux permits renaming
an open directory, so the Windows rename-refusal assertion is not reused there.
These are Windows-host Linux-container results, not native Linux-host acceptance.

The extended fixture also causes an actual OS close failure on each target.
Windows obtains the generated TEMP database handle through the public
[SQLite native-handle control](https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html#sqlitefcntlwin32gethandle).
It applies
[`HANDLE_FLAG_PROTECT_FROM_CLOSE`](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-sethandleinformation)
only to that handle. Linux uses a thread-local seccomp filter to refuse `close`
on the exact descriptor returned for its owned TEMP facade. It needs no added
container capability, host setting, installation or elevated command.

Both `sqlite3_close_v2` calls return success. On Linux, even the upstream file
method returns success while the descriptor remains open. Counting completed
callbacks is therefore insufficient. The compile-time bridge now observes
`osClose` and `osCloseHandle` directly, preserves native errors and leaves
upstream locking and close logic intact. Runtime-hook mode observes the same
calls for its comparison run. No SQLite log callback or message matching is used.

Rust reports `Release::Quarantined(SQLITE_IOERR_CLOSE)` and emits a redacted
diagnostic. The native context retains its real authority handles and refuses a
new SQLite open before any native open call. The test verifies the exact leaked
file handle or descriptor, not just a failure counter.

Only explicit fixture cleanup releases the known fault: Windows removes its
test-only close-protection flag; Linux uses `close_range` on the single
identity-checked, syscall-refused descriptor. This is not a general recovery
algorithm. Do not retry an uncertain
[Linux close](https://man7.org/linux/man-pages/man2/close.2.html).
The Linux filter stays on that fixture thread, so it admits no later context.
The recorded failure stays a failure after fixture cleanup.

Run both targets with the existing isolated image and installed compilers:

```powershell
.\scripts\storage-sqlite-prototype.ps1 -Platform Both -Adapter PinnedSource -RustOwnership
```

Both targets release five contexts after the explicit cleanup. Each still
verifies all 8,192 statement rows, 8,192 backup rows, 1,032 BLOB bytes and 16,384
owner-thread rows. These prove one active, thread-confined context, not a general
native registry or an approved production threading design.

Final source-matched run `bbd5be0d0262468294118564df22155b` is recorded in
`files/storage-sqlite-resolved-ownership-final-proof.json`. Both the current
PowerShell host and Windows PowerShell complete the expected-failure test.
The early log-observation experiment is historical only; final evidence uses
the direct native close bridge.

#### Explicit SQLite entry policy

A new regression first reproduced a valid-name bypass:
`file:state.sqlite?immutable=1` passed the earlier filename guard.
[SQLite documents](https://www.sqlite.org/uri.html) that `immutable` disables
locking and change detection, `nolock` disables lock calls, and `mode=memory`
can avoid filesystem access entirely.

The diagnostic state entry now accepts only its exact fixed database token and
explicit read-only, existing-write or create flags. It refuses memory names,
URIs and additional flags before SQLite parses them. The scoped VFS separately
rejects URI parameters through the public `sqlite3_uri_key` interface.
The entry policy must remain outside the VFS too: a memory database need not
call the VFS at all.

A connection authorizer rejects named `ATTACH` destinations and changes to
SQLite's process directory settings. One empty-name private scratch attachment
remains available for plain `VACUUM`. The attached-database limit is one.
The controlled in-memory backup destination stays separate from state entry.
Native extension loading is disabled for state connections.

The caller file-control facade allows only the read-only data-version query.
It refuses handle replacement, disabled I/O and raw file/VFS handle access.
Internal SQLite methods and the explicit fixture introspection still use the
upstream methods. This caller restriction does not remove required SQLite
maintenance or file-method behavior.

Each target passes 18 entry-policy refusals, including three direct VFS URI
refusals. Exact committed keys and the attached schema list remain unchanged.
Existing disk TEMP/spill, plain `VACUUM`, subjournal, transient materialization,
read-only WAL snapshots, contention, recovery and link/refusal checks still pass.
Raw SQLite connections, filenames and file controls must not escape the final
owner API. This fixture is not a general sandbox for arbitrary native extensions
or untrusted SQL.

| Current mode | Linux parent assertions | Windows parent assertions |
| ------------ | ----------------------- | ------------------------- |
| PinnedSource | 365                     | 1,130                     |
| RuntimeHooks | 512                     | 1,132                     |

These are assertion counts, not coverage metrics. Final source and hook runs are
`50433c3712f24beb82c70bc12d1efa18` and
`d37cf8e419cf4f42a9c89ec4230f1841`, recorded in
`files/storage-sqlite-entry-policy-direct-source-proof.json` and
`files/storage-sqlite-entry-policy-direct-hooks-proof.json`. All normal workflows
report zero actual native close errors. All three final runs record source hashes
and removal of their generated resources.

**Remaining boundary:** These results close the bounded SQLite ownership,
native-close failure and entry-policy proof tasks. They do not prove persistent
root/backing authority, the required Intel member mapping or general remote file
operations. The following native Ubuntu result covers the bounded SQLite fixture
only. The production callback/owner API and saved-binding contract still need
review before runtime integration.

#### Native Ubuntu preparation attempt

The approved disposable Ubuntu 24.04 x86_64 Azure VM passed the resource,
inbound-deny and automatic-shutdown checks. An independent cleanup process was
active before resource creation. The guest verified Rust 1.99.1, uv 0.13.8,
the pinned SQLite archive and the original SQLite source. The attempt stopped
at the patched-source hash check, before compiling or running the SQLite
proofs. Azure reported successful command execution, but the proof did not
return its required acceptance record. This is not a passing native Linux test.

An offline replay found the cause. Windows Git writes the patched source with
CRLF line endings; Linux Git writes LF. The Windows result has SHA256
`440C51BEEDCB577FE9F655EDAD65A192123D1FF3445EEC9578BA634F81052D5C`.
Converting only its line endings to LF produces SHA256
`85889663850FD054B019E9F1CB9A0668218A13D8BA42BF82C645EBD8B4F6C009`,
which matches a fresh Linux patch application. The original source and patch
hashes are unchanged. The guest runner now checks the verified Linux digest
and prints the actual digest for failure diagnosis.

Private evidence is recorded in
`files/native-linux-response-006.json`,
`files/native-linux-offline-patch-replay.txt` and
`files/native-linux-patch-line-endings-proof.json`. The controller and independent
watchdog both completed cleanup. A separate Azure query confirmed that the
task-owned resource group was absent.

The user approved one replacement within the original shared budget and
deadline. It confirmed the corrected Linux digest and compiled both ordinary
SQLite adapter executables. It then stopped before tests: the ownership fixture
includes `probe.c`, but the guest runner had not created that source alias.
The runner now copies the exact original prototype to that required name.
The replacement's resource group was also removed.

A full offline ext4 run then compiled the exact corrected runner commands and
passed both adapter workloads and Rust ownership as UID 1000 with zero effective
capabilities. It retained the strict ext4 check; an earlier tmpfs preflight
correctly failed that check. The ext4 run passed 365/512 parent assertions,
18 entry-policy refusals and six namespace refusals per mode. It released all
five ownership contexts after the explicit close-failure recovery. The generated
Docker volume was removed and its absence was checked.
This remains Windows-host Linux-container evidence, not native Ubuntu acceptance.
The raw record is `files/native-linux-full-offline-ext4-preflight.txt`.
The following section records the separately approved final Ubuntu attempt.

#### Native Ubuntu SQLite acceptance

The final approved Ubuntu 24.04 x86_64 Azure VM passed both SQLite adapter
workloads and Rust ownership on its native ext4 filesystem. The probes ran
directly as UID 10001 with zero effective capabilities, not inside Docker.
The guest used GCC 13.3.0, Rust 1.99.1 and kernel `7.0.0-1017-azure`.
The original shared USD 5 budget and two-hour deadline were not increased.
A stricter 20-minute proof limit stayed below the approved 35-minute maximum.

PinnedSource and RuntimeHooks passed 365 and 512 parent assertions. Each mode
also passed 18 entry-policy refusals, three VFS URI refusals and six namespace
refusals. Exact committed values, TEMP/spill, WAL contention and snapshots,
auxiliary-file lifetime, recovery and link/refusal checks remained active.
Both normal modes reported zero actual native close errors.
Assertion counts are diagnostics, not coverage metrics.

Rust ownership verified all 8,192 statement rows, 8,192 backup rows, 1,032 BLOB
bytes and 16,384 owner-thread rows. One actual native close failed. The context
remained quarantined and refused new admission until explicit fixture recovery.
All five native contexts were released afterward.

The host independently matched all seven transferred source hashes, three
control artifacts, actual VM identity and exact ownership outcomes. The
controller and independent watchdog completed cleanup. A separate Azure query
confirmed that the task-owned resource group was absent.
The source-matched record for run `718c4d94d6ac4878a58c6a731bb76915`, attempt three,
is `files/native-linux-attempt-3/storage-sqlite-native-ubuntu-proof.json`.
Independent validation is saved beside it as
`storage-sqlite-native-ubuntu-validation.json`.

**Limits:** This closes native Ubuntu execution for the bounded SQLite fixture.
It does not prove physical backing identity, the Intel member contract, durable
startup bindings, general remote operations, bare-metal behavior, reboots or
application API/worker/CLI integration. Production Rust and dependencies remain
unchanged. The separate production supported-contract review is still required.

#### Frozen configuration selection

Resolve root selection and `<application-storage>/config.json` once at startup.
Keep the resulting snapshot immutable. The same selection rules apply to all
processes and commands. Shared administrator-controlled output policy stays
versioned in SQLite, not host JSON.

Normal components can read/inspect/validate host configuration but cannot save it
or reload only the API. Coordinated stop/edit/validate/restart activates changes.
Passive inspection must not create roots, migrate SQLite or initialize providers.
First-run creation is create-once; an invalid existing file remains unchanged.
Explicit fresh setup uses fresh state without deleting or moving old data.
Preserve secure startup failure, read-only command guards and bounded I/O.

### Remaining proof gates

Proceed in dependency order. Design the database adapter and bootstrap protocol
first. Define and verify logical provider/root continuity within the trusted
operator mapping before bootstrap acceptance. Define exact topology assurances
for supported opaque provider mappings; do not require a physical backing graph.
General remote operation proof follows the same frozen bindings and native
parent authority. Each new effect needs the existing application intent, claim
and approval protocol.
Neither a metadata witness nor a signed operation result replaces those checks.

Reuse only approved fresh fixture resources and installed tools for these proofs.
Read-only collection from existing physical disks, a new server installation,
account/ACL changes, elevated mounts or media mutations still need their own
specific approvals. Do not treat the user-directed continuation as those host
permissions. Present the exact supported contract and unresolved exclusions before
the separate production integration approval.

| Gate                       | Evidence required before integration                                                                                                                                                                        |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Different native volumes   | Verify native root identity in each supported logical provider scope. Check namespace relationships or exact operator topology assurances. Do not require physical-device independence.                     |
| Client-local versus remote | Define and test exact operator assurances for opaque private/media mappings. Refuse missing or stale assurances and known overlaps. Do not claim independently proved separation from administrative input. |
| Durable remote bindings    | Define provider-scoped identity, reconnect/remount behavior, inventory changes and explicit pin/root rotation. The transient diagnostic context is insufficient.                                            |
| Network operations         | Prove production operations against frozen bindings and retained native parents. Bounded read-only lab cases do not prove reusable child leases, arbitrary streams, effects or recovery.                    |
| Protected SQLite files     | Prove main-file and sidecar access, parent replacement refusal, compatible multiprocess locks and crash/recovery behavior on both native OS targets.                                                        |
| Root bounds and names      | Set production inventory/response limits separately from fixture limits. Verify normal Unicode root names and filesystem case rules. The bounded ASCII prototype is not a product-wide path restriction.    |

Historical Windows
[volume disk extents](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_volume_get_volume_disk_extents)
and
[storage dependency information](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/nf-virtdisk-getstoragedependencyinformation)
were investigated for different-volume/backing review.
The latter describes virtual-disk/parent-volume relationships.
Neither is a prerequisite under the confirmed operator-trusted contract.
The investigation records below preserve their original findings and limitations.
Their unresolved physical-backing requirements are superseded by that contract.
No new host volume, mount, account or installation changes are authorized.

### Approved read-only C: backing metadata

The user separately approved read-only metadata collection from existing `C:`,
which holds the generated Windows fixtures. This approval permits at most eight
device records and 64 KiB per native response. It does not permit file-content
or media reads, elevation, mounting, installation, account changes or permission
changes. Hardware identifiers remain in private session evidence.

```powershell
.\scripts\storage-windows-backing-metadata.ps1 -ParserOnly
.\scripts\storage-windows-backing-metadata.ps1 -EvidencePath <new-private-evidence-file>
```

The native command requires that specific collection approval. Parser-only mode
uses generated bytes and performs no native collection. The evidence destination
must have an existing parent outside the repository. An existing destination is
refused before collection; evidence is written with create-new semantics.
Each invocation uses a fresh PowerShell process so the recorded source hashes
identify the compiled collector.

The collector retains and rechecks the C: root and correlates its full native
identity with the matching volume-GUID root. It reads actual volume extents,
opens only their derived device numbers with desired access zero, and requires
the returned device number to match. Storage properties 0, 2 and 3 use fixed
64 KiB buffers. Unavailable and over-bound responses remain explicit.
The volume extent snapshot and both retained roots are rechecked before return.
All native handles are disposed.

Current-source run `1f6073f9dcd34704a047cbe7a01272f4` observed one correlated
device. Property responses were 396, 48 and 464 bytes respectively.
The root identity and volume coordinate matched the earlier collection in a
different process. The DUID response reports version 1, with device-ID and
device-descriptor components but no layout component. The device-ID descriptor
reports one binary, device-associated type-2 identifier with a 16-byte payload.
These are format observations, not a proved durable identity profile.
No raw identifiers or identifier hashes are published.

The dependency API returned `0xC03A0015`, "The specified disk is not a virtual disk."
This is an unavailable dependency result, not proof of a complete backing graph
or separation. No binding was admitted. The parser accepts the exact eight-device
and 64 KiB boundaries and refuses seven malformed inputs, including truncated
extents and overflowing ranges.

Private current-source evidence is
`files/storage-c-volume-metadata-current-private.json`.
The earlier run `57246f4981c74e9b860b5d900569adcd`, in
`files/storage-c-volume-metadata-private.json`, retains its original harness hash.
The later harness adds boundary tests and destination preflight; do not assign
its hash to the earlier record. The existing VHD collectors remain unchanged.

**Identity limits:** Metadata availability and fresh-process root continuity do
not prove reboot behavior, clone refusal or persistent bootstrap admission.
Microsoft's
[DUID guidance](https://learn.microsoft.com/windows-hardware/drivers/storage/device-unique-identifiers--duids--for-storage-devices)
requires
[`CompareStorageDuids`](https://learn.microsoft.com/windows-hardware/drivers/ddi/storduid/nf-storduid-comparestorageduids),
not byte equality, for DUID comparisons. Exact match, sub-ID match, no match and
errors are distinct. Layout signatures cannot distinguish a LUN from its
snapshot; not every device supplies sufficiently unique information.
`StorageDeviceUniqueIdProperty` is documented for driver use.
The installed SDK include trees contain no `storduid.h`; no comparison helper
was copied, reimplemented or installed. The collector does not compare DUIDs
or accept sub-ID matches as authority.

The exact durable local identity profile, complete supported backing authority,
common local/remote separation and separate production integration approval
remain open. This collection changes no production Rust, dependencies,
configuration or existing storage.

### Selected strict device-ID descriptor proof

The user selected a strict device-ID descriptor profile instead of a new DUID
comparison dependency. This next proof reads generated descriptors and the
existing approved private C: record only. It performs no native collection.

```powershell
.\scripts\storage-windows-device-id.ps1
.\scripts\storage-windows-device-id.ps1 -MetadataEvidencePath <private-C-metadata-file> -EvidencePath <new-private-proof-file>
```

The offline codec requires descriptor version 16, an exact response size,
at most eight identifiers and a 64 KiB response bound. It checks each header,
payload and next offset before selection. Unknown enum values, duplicate
supported identifiers, empty/sentinel identifiers and malformed extents fail.
Only binary, device-associated type-2 designators of eight or sixteen bytes are
selected. Known vendor, port and text identifiers cannot replace a supported
identifier. Matching uses the complete supported inventory without dependence
on order. An added/removed identifier, different payload or different width
does not match; an eight-byte prefix is not a sixteen-byte identity.

The
[identifier-type documentation](<https://learn.microsoft.com/en-us/previous-versions/windows/hardware/drivers/ff566990(v=vs.85)>)
describes EUI-64. The current
[NVMe namespace documentation](https://learn.microsoft.com/en-us/windows/win32/api/nvme/ns-nvme-nvme_identify_namespace_data)
also defines a sixteen-byte EUI-based designator for NGUID.
The initial eight-byte-only codec refused the recorded sixteen-byte descriptor.
That historical offline result remains in
`files/storage-device-id-offline-proof.json`.
The extended codec keeps the two documented widths distinct. It does not infer
that the recorded value is an actual NVMe NGUID.

Final run `a297d0b5d6e14ad29907c6306e0289a6` passes 30 malformed/unsupported
refusals, exact eight-identifier and 64 KiB boundaries, both width checks,
inventory reordering and changed-inventory refusal. Windows PowerShell 5.1 run
`b7fc046fd4c24e208c98932c92ebc73a` passes the same cases on the same sources.
The recorded C: descriptor has a candidate-supported format; no binding is
admitted. Its original private evidence remains unchanged.
Final redacted records are `files/storage-device-id-width-proof.json` and
`files/storage-device-id-width-windows-powershell-proof.json`.

The C: device descriptor reports a RAID bus, not a directly exposed NVMe bus.
A device-associated identifier can identify a presented logical unit without
describing its underlying storage. Neither the descriptor nor the virtual-disk
API's unavailable result establishes a complete RAID backing graph.
No disk/controller identifiers are published.

**Limits:** These are descriptor-format and exact-matching tests, not native
clone/reboot acceptance, a serialized bootstrap schema or backing authority.
Generated changed identifiers are not physical clone tests. Private JSON is
offline input, never an admission credential. The actual presented device,
its complete supported backing relationships, root identity and operator
expectation still require separate proof before SQLite admission.
No DUID blob comparison, runtime fallback or production integration is added.

### Approved C: storage and controller topology

The user separately approved bounded read-only metadata for C:'s directly
related storage and controller nodes. The limits remain eight records and
64 KiB per native response. Hardware values remain private. No content/media
reads, elevation, installation, mounts or permission changes are authorized.

```powershell
.\scripts\storage-windows-topology.ps1 -ParserOnly
.\scripts\storage-windows-topology.ps1 -EvidencePath <new-private-evidence-file>
```

The collector takes a fresh C: native observation. An exact-index CIM query
supplies only a PnP seed, not authority. The seed must resolve to one present
disk interface. An access-zero native handle must return both the exact device
number and the observed device-ID descriptor. The collector walks only that
device's parent chain and stops at the controller, an unsupported class,
unavailable metadata or the record bound. It never enumerates all controllers
or opens unrelated disk interfaces.

Configuration Manager responses use fixed buffers. Instance IDs, classes,
services, parent links and the interface mapping are rechecked.
[`CM_Get_Parent`](https://learn.microsoft.com/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_get_parent)
describes PnP ancestry, not physical storage membership.
The
[device-filtered interface query](https://learn.microsoft.com/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_get_device_interface_listw)
avoids an all-device inventory. The CIM seed is limited to one index and two
properties; its ID length is bounded before native use.

The same retained device-interface handle queries
`StorageDevicePhysicalTopologyProperty` (54). This constant and structure sizes
were checked against the installed Windows SDK. The
[physical topology descriptor](https://learn.microsoft.com/windows-hardware/drivers/ddi/ntddstor/ns-ntddstor-_storage_physical_topology_descriptor)
uses a separate bounded decoder. It checks version, total size, node and member
counts, member-data extents and nonoverlap. The combined PnP and physical record
count cannot exceed eight. Unavailable metadata and insufficient record budget
remain explicit, with no larger-buffer or privileged fallback.
The physical response is queried twice and must remain byte-identical.

Final run `3d25a05a0a404ec29250f3f8284a8342` observed two PnP records:
`DiskDrive` with service `disk`, then `SCSIAdapter` with service `iaStorAVC`.
The 456-byte physical response contains one node, one adapter and two devices.
Both device roles are data; both command-protocol values are 2 (ATA).
This totals six records across both views, within the approved eight-record
limit. Device/controller IDs, models and other hardware values remain private.

The actual interface number/descriptor correlation, parent/property rechecks
and surrounding C: root/device observations match. All native handles are
disposed. Root handles are not retained continuously across the surrounding
observations; this is explicitly not an admission lease or an atomic snapshot.
The two physical members are observations from the controller, not an
independently proved complete backing graph.

The parser passes 22 malformed-response refusals, exact 64 KiB string/interface
boundaries and an exact eight-record physical fixture. Existing evidence output
is create-once. Final source-matched private evidence is
`files/storage-c-topology-final-private.json`.
Earlier `files/storage-c-topology-private.json` and
`files/storage-c-topology-current-private.json` retain their original source
hashes; neither contains the final retained raw physical response.

**Remaining gate:** The
[physical device structure](https://learn.microsoft.com/windows-hardware/drivers/ddi/ntddstor/ns-ntddstor-_storage_physical_device_data)
reports a device ID, role, protocol, model and capacity. These fields do not by
themselves establish a durable provider-scoped member identity or correlate
members across controllers and providers. The reported RAID bus and two members
must not be reduced to the presented disk's single EUI-based identifier.
The evidence does not establish a RAID level, clone resistance, reconnect/reboot
continuity or client-local/remote separation. No binding is admitted.
Production Rust, Store, configuration and dependencies remain unchanged.

### Required Intel controller member authority

**Historical requirement, now retired:** The operator-trusted filesystem
contract removes Intel/RAID membership from production prerequisites.
The captured response and decoder limitations below remain unresolved evidence;
they are not an accepted ABI and do not require further investigation for #68.

The earlier investigation required this controller in the first supported
Windows profile. Its observed two-member topology did not establish durable
membership or independently measured common-provider separation.
Retiring that proof requirement does not remove the Windows deployment from scope.

The public [Intel SSI project](https://github.com/intel/irstessi) is archived.
Its [current README](https://github.com/intel/irstessi/blob/master/README.md)
states that Intel discontinued maintenance. Its
[installation README](https://github.com/intel/irstessi/blob/master/README)
describes a Linux RSTe shared library that uses `mdadm`, not a validated Windows
adapter. It also warns that loading the library overwrites `mdadm.conf`.
Do not load that library as an effect-free metadata probe, install it, or adopt
it as the Windows dependency.
An [Intel support response](https://community.intel.com/t/publicly-available-api-documentation/33946)
directs API questions to developer support; it does not supply a current
supported Windows member-identity contract.

An alternative source-backed investigation is CSMI through the existing Windows
driver. The
[smartmontools Windows implementation](https://github.com/smartmontools/smartmontools/blob/a3aee76ed7f342efebd5f01631be1d79f7fdb780/lib/os_win32.cpp)
uses a controller device handle with `GENERIC_READ | GENERIC_WRITE` and
`IOCTL_SCSI_MINIPORT`. Driver information, RAID information and PHY information
are separate from SATA pass-through. This is implementation evidence, not
Intel's approval of a stable interface or proof of this controller's support.
The installed PATH has no `smartctl`; no diagnostic software was installed.

The completed topology proof used access-zero handles and standard metadata
queries. It does not approve raw miniport commands or a read/write-capable
controller handle. Any next native CSMI proof needs a separate bounded scope:
correlate only C:'s actual controller before opening it, permit only specified
metadata commands, keep the eight-record/64 KiB limits and private evidence,
and stop on access denial. Never scan controller numbers, enable registry
settings, issue media commands, reset the controller or elevate as a fallback.
No CSMI calls have been made. Durable member mapping remains blocked.

#### Approved non-elevated CSMI probe

The user then explicitly approved an isolated metadata-only CSMI probe. This
permits a read/write-capable handle to C:'s verified controller and only command
1 (driver information), 10 (RAID information) and 20 (PHY information).
It does not permit elevation, other commands, media reads, resets, registry
changes, installation, mounts or permission changes.

```powershell
.\scripts\storage-windows-csmi.ps1 -ParserOnly
.\scripts\storage-windows-csmi.ps1 -EvidencePath <new-private-evidence-file>
```

The collector requires the actual two-node C: parent chain and the observed
`SCSIAdapter`/`iaStorAVC` controller profile. A disk interface is corroborated by
its native number and exact descriptor. The controller interface query is
filtered to that verified parent. No controller numbers are guessed or scanned.
The [miniport documentation](https://learn.microsoft.com/windows-hardware/drivers/ddi/ntddscsi/ni-ntddscsi-ioctl_scsi_miniport)
also warns that not every driver creates a numbered SCSI alias.

The command builder has only the three approved metadata requests. It provides
no generic miniport or pass-through operation. Fixed request/response sizes are
204, 128 and 2,080 bytes. Headers use the SDK's `SRB_IO_CONTROL` layout.
Response signatures, command IDs, lengths, replaced status, string termination
and record limits are checked. Driver refusal is distinct from Win32 failure.
Two summary records and at most six declared PHY records fit the eight-record
budget. Undeclared nonzero PHY slots fail rather than triggering inferred port
mapping. Each request specifies a five-second driver timeout; no hard driver
completion deadline has been proved.

The generated parser passes 19 allowlist/malformed-response refusals, all three
metadata frames, explicit driver-error handling and the exact six-PHY boundary.
These are generated tests; they do not prove this driver's response format.

Native Windows PowerShell 5.1 run `76bc18a1b2354a3daec09cff8134fca6` found the
exact controller interface but its read/write-capable open failed with Win32
error 5, `Access is denied`. **Zero CSMI commands were sent.**
The collector did not retry, elevate, change permissions or use another alias.
Surrounding root/device/parent/topology observations still matched.
All opened native handles were disposed. No binding was admitted.

Private source-matched evidence is `files/storage-c-csmi-private.json`.
No hardware values are published. Controller command support, actual driver
response validation, durable member identity and complete backing authority
remain unproved. A new elevated attempt would require separate explicit
approval; the current approval requires stopping on access failure.

#### Separately approved single elevated run

The user then explicitly approved one elevated run of the same metadata-only
probe. The prior access-denied record remains unchanged. The elevated wrapper
verified all probe source hashes against that record before execution.
Installed Windows PowerShell 5.1 performed the single run; no tools, accounts,
ACLs, registry settings, mounts or services were changed.

Run `838dff261b294741a91901ac41744a7a` correlated the actual C: disk and parent
controller and completed only the three approved queries. Each returned Win32
success and CSMI return code zero, with exact response sizes 204/128/2,080 bytes.
Native disk/parent/interface checks and surrounding root/topology observations
passed. Handles were disposed and the elevated process exited with code zero.
No binding was admitted.

The driver reports CSMI version 0.81, one RAID set and four PHY records.
All four attached-device type fields report end devices with SATA protocol.
Three attached address fields are nonzero and distinct; one is zero.
Array position does not consistently match the reported PHY identifier.
Do not treat slot numbers or addresses as durable identities or invent the
mapping from these four controller-wide records to C:'s two physical members.
No raw addresses, IDs or identifier hashes are published.

The reference header is version 0.90. Its history notes RAID-configuration
layout incompatibility between 0.87 and 0.89. The current proof checks the
three measured metadata envelopes and selected prefix fields only; it does
not approve interpreting a newer RAID-configuration layout against this
0.81 driver. Command 11 (RAID configuration) and all pass-through commands
remain outside the approved allowlist. No such commands were sent.

Private evidence is `files/storage-c-csmi-elevated-private.json`, with the
wrapper result in `files/storage-c-csmi-elevated-status-private.json`.
Probe source hashes match both the non-elevated and elevated records.
This closes the actual metadata-query access proof, not durable member mapping,
physical clone/reboot acceptance, complete backing authority or production
integration. Any additional native command or elevated run requires a new
explicitly bounded approval.

#### Historical member-query reference

A pinned
[historical Intel QST header mirror](https://github.com/gigaplex/Intel_QST_SDK/blob/db5dce39a1b2e245bd5e3becc67585e05f4456a6/src/Libraries/Windows/CSMIIOCTL.h)
defines an earlier command-11 layout with a 36-byte configuration prefix and
128-byte member records containing model, firmware, serial and address fields.
Unlike the newer header, it has no later per-member extension fields.
This is a candidate reference, not a current Intel-supported API or proof that
the 0.81 driver uses that exact layout.

The matching
[historical caller](https://github.com/gigaplex/Intel_QST_SDK/blob/db5dce39a1b2e245bd5e3becc67585e05f4456a6/src/Libraries/Windows/CSMISMART.c)
queries command 11 with the RAID-set index and bounded member storage.
It also contains address workarounds and calls a SMART-enabling function.
Do not run that sample, copy its address guesses or enable device monitoring.
No historical SDK code was loaded, executed or installed.

The isolated `scripts/storage-windows-csmi-raid.ps1` now tests that candidate
offline. Its independent C# decoder uses the referenced protocol offsets, not
copied vendor implementation code. It accepts generated two-member and six-member
responses, fixed-width padding and zero unused member capacity. It rejects 18
malformed or unsupported cases, including duplicate or missing model/serial
tuples, invalid encoding, partial records, count/size mismatch, prefix/member
extensions, undeclared data and the newer version. Both PowerShell engines pass.
The decoder requires the observed 0.81 version as an explicit input, but this
does not prove that the real driver implements the candidate layout. It does not
interpret RAID type, status or address fields as authority.

The offline proof has no native command sender. Any next command-11 proof must
preserve the bounded decoder and fail on an unsupported layout. It must not infer
that the only RAID set is C:'s set or that a member serial alone proves backing
separation.

#### Separately approved member probe and canceled elevation

The user then approved one elevated metadata-only probe. The separate
`-RaidConfiguration` path rechecks commands 1/10 before command 11 once for set
index zero. It requires the same saved C: root, disk descriptor, extents and
controller observations. Driver and RAID responses must match the prior approved
private evidence exactly. The version must remain 0.81, with one RAID set and at
most six members per set. Only the reported member capacity is allocated.
Two summaries plus six member records fit the eight-record bound.

The default collector remains limited to 1/10/20. The dedicated member path has
no generic command sender, pass-through, address workaround or SMART-enabling
operation. It preserves raw responses privately before reporting malformed or
unsupported responses. It never admits a storage binding.

Both PowerShell engines pass the 19 existing parser refusals and ten new
request/prerequisite refusals. Exact one-, three- and six-member capacity
boundaries pass. The three actual payloads from the earlier successful run also
pass offline. Its RAID information reports a maximum of three members per set.
These are decoder checks, not a new native query.

The single `Start-Process -Verb RunAs` attempt was canceled by the user before
the elevated process started. Neither native evidence nor wrapper output existed
after cancellation. No command 11 ran, and no retry was attempted. The parent
recorded the cancellation in the create-once private status file, so the wrapper
cannot be reused. Actual driver compatibility and member mapping remain blocked.
Another elevated attempt needs a new explicit approval.

#### Separately approved member retry

The user explicitly requested another attempt after the canceled launch.
The new create-once wrapper preserved the old cancellation record and used
the same tested source pins and command limits. Its single elevated process
exited zero. Run `ef44ff04e1a245daaa6be9e25e87eb38` completed only commands
1/10/11, with Win32 success, CSMI return code zero and response sizes
204/128/448 bytes. Driver and RAID responses matched the approved prior evidence.
Controller, disk and surrounding observations were rechecked. All recorded
source hashes match the tested sources.

The command-11 response contains nonzero data in the historical configuration
prefix's reserved area. The strict decoder reports
`Historical RAID prefix extension is unsupported.`
It does not accept a member count or admit a binding. The exact refusal is
reproducible offline from the preserved response. Do not ignore or zero these
bytes to force the candidate layout to pass.

Private evidence is `files/storage-c-csmi-raid-retry1-private.json`, with the
wrapper result in `files/storage-c-csmi-raid-retry1-status-private.json`.
The bounded native execution is complete, but the historical candidate layout
does not pass the actual response. Member mapping and durable backing authority
remain unproved. No additional elevated run is approved.

#### Offline member-layout investigation

The preserved response has an unaccepted count of two. Its second printable
member-field sequence starts at offset 200, not the historical candidate's 192.
The [pinned newer smartmontools header](https://github.com/smartmontools/smartmontools/blob/a3aee76ed7f342efebd5f01631be1d79f7fdb780/lib/csmisas.h)
and [CrystalDiskInfo header](https://github.com/hiyohiyo/CrystalDiskInfo/blob/cabe686f0b09a8d0a777a24b410c0041630010ad/AtaSmart.h)
define 136-byte members, rather than the historical 128-byte members. This
explains a possible field-boundary difference, not a proved layout for this
driver. The newer header also changes the prefix fields and reports version
0.90. Do not select it automatically for the observed 0.81 driver.

The [matching CrystalDiskInfo caller](https://github.com/hiyohiyo/CrystalDiskInfo/blob/cabe686f0b09a8d0a777a24b410c0041630010ad/AtaSmart.cpp)
uses its newer structure while reading RAID configuration. Its broader path
also sends identification/pass-through commands and applies device-specific
address handling. None of that code was run, installed or adopted. It does not
supply the missing supported Intel prefix contract.

The offline harness now has 19 generated refusals, including shifted member
fields within an otherwise bounded old-size allocation. Its optional
`-InputEvidencePath` audit opens the saved evidence read-only, with a 1 MiB input
bound. The original captured response fails at the prefix. An adversarial
in-memory copy with only that prefix cleared still fails at the member extension.
This tests refusal of a proposed unsafe workaround; it does not change the
original response or relax the decoder. Both PowerShell engines pass, and the
private evidence remains byte-identical.

The codec source and native collector remain unchanged. The updated offline
harness SHA-256 is
`3B45E2F1A65E46C080810DC6F1B8C91AE094D35757D5AB79556E66A12E729CF3`.
Earlier hashes remain evidence for their exact earlier revisions. A documented
version-specific layout and mapping are still required before member admission.

### Confirmed initial remote-media profile

The user selected **Require a proved server-side adapter** for remote write
support. Standard SMB/NAS access without proved native witness and operations
is not an approved write fallback. The user later approved scan-only SMB/NFS
libraries through the host filesystem client. Those roots still require the
supported identity codec and exact topology assurances. Supporting every NAS
is not claimed.

Keep runtime integration blocked until the applicable gates pass and the
separate integration approval is given. The profile decision does not approve
runtime code, a production package, installation or deployment.
The experimental Samba build is not a selected production version.

### Measured Windows different-volume observations

The user separately approved two generated 256 MiB VHDX images, with a 512 MiB
total virtual-capacity limit. The final run passed 24 assertions on actual
NTFS volumes. The two dynamic backing files had a total file length of 72 MiB.
No drive letters or permanent attachments were used.

The collector queried full root identities, volume extents and storage
dependencies. Each extent matched its exact retained VHD handle. Each returned
backing-file identity and coordinate matched the retained original image file.
The actual wrong-image query failed. Volume aliases overlapped, and child
directories opened through retained native parents.

Both image files were inside the generated host private-directory candidate.
The mounted roots had different volume GUIDs, but their backing bytes remained
inside that candidate. The existing rule returned unknown, not separated, for
both cross-volume directions and the host/image containment cases.
**Different volumes still do not establish unconditional separation.**

Disk initialization changed `GET_VIRTUAL_DISK_INFO_IDENTIFIER` (version 2).
The revised ownership check retains the image file's full native identity and
checks `GET_VIRTUAL_DISK_INFO_VIRTUAL_DISK_ID` (version 14) through the original
VHD handle. Version 2 values were recorded before/after initialization;
version 14 was rechecked after each disk operation.
These [documented identifiers](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-get_virtual_disk_info_version)
are diagnostic ownership checks, not globally clone-resistant root authority.

The dependency flags were `0xc0`: removable backing and no automatic drive
letters. The collector accepts removable backing only when its native volume
matches the running OS volume and the OS drive reports fixed local storage.
It rejects other/unknown dependency flags. The images are dynamic, so the
[`FULLY_ALLOCATED` flag](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-dependent_disk_flag)
is not required. Do not copy the fixed-volume fault-lab requirement into this
different diagnostic.

A controlled exception detached the first image before partitioning. The same
image then reattached. Both final images detached through their retained handles,
reported unloaded, and were removed. Independent checks found both generated
volume names unavailable and all five exact attempt directories removed.
The first failed attempt left a detached image; its detachment was independently
checked before exact cleanup. Later failures cleaned up normally.

The historical final source hashes are recorded in
`files/storage-vhd-efb194ab15bb4ef6bdaeeff44f809097.json`.
The original 24 same-volume assertions also pass on the current root collector;
their result is `files/storage-windows-current-proof.json`.
PowerShell parsing and C# compilation pass under Windows PowerShell 5.1.
The installed PowerShell 7 elevation path crashed before executing the preflight;
the installed Windows PowerShell path performed the approved disk operations.
No software was installed.

This is elevated, generated metadata evidence. The backing-path corroboration
is not general retained-parent file-effect authority. A positive cross-volume
relationship rule, durable identity, nonprivileged collection, physical-disk
acceptance and production database/media operations remain unproved.
No runtime bindings changed.

### Measured saved Windows image binding

The user separately approved the next two-slot VHDX proof. Each generated image
slot has 256 MiB virtual capacity. Only two slots exist at once; the second
original is detached, closed and removed before its path holds a copy.
No drive letters, permanent attachment, existing-disk/media changes, accounts,
ACL changes, services, boot changes or installations.

Use the existing elevated harness with `-SavedBindingProof`:

```powershell
.\scripts\storage-windows-volumes.ps1 -SessionFiles <session-files> -Run <fresh-guid-N> -SavedBindingProof
```

Current run `f7868b7f3feb4c8890e4cd46d7d3cde5` passed 40 assertions.
It includes the original actual extent/backing association, alias, child,
wrong-image and host/image containment cases. Different-volume relationships
still return unknown. The final two backing files totaled 36 MiB.

A create-once generated `config.json` stores the first root's full native
identity, retained backing-file identity and version-14 virtual disk identifier.
This is a diagnostic expectation, not a production schema or provider scope.
After closing every native root handle, both images detach. The first reattaches;
a newly opened native root reports the same full identity and maps through its
actual extent/dependency to the exact saved backing file. Parent-relative child
work succeeds. A different actual image cannot use the saved first-image pin.
The original virtual-disk owner handle remains retained across this lifecycle.
This does not prove process restart or reboot.

After final detach, the second slot receives a byte-identical copy of the first
image. SHA-256 matches before and after inspection. Actual read-only
`OpenVirtualDisk`/`GetVirtualDiskInformation` reports that the copy preserves
both identifier versions 2 and 14. Its full native backing-file identity differs.
The saved original pin refuses the copy before attachment; the copy remains
unloaded. Configuration bytes and copied-image bytes remain unchanged.
**A copied virtual disk identifier is not independent backing authority.**

The copy uses documented version-one open parameters with read/write depth zero,
GET_INFO-only access and no parent traversal. There is no attachment, disk
initialization or write operation on the copied image.
See [OpenVirtualDisk](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/nf-virtdisk-openvirtualdisk),
[open parameters](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ns-virtdisk-open_virtual_disk_parameters)
and [access masks](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-virtual_disk_access_mask-r1).

All exact original/copy images and the generated configuration/fixture are
removed. Both generated volume roots are independently unavailable. The run's
source hashes match `files/storage-vhd-f7868b7f3feb4c8890e4cd46d7d3cde5.json`.
The unchanged original same-volume root proof passes 24 assertions, recorded in
`files/storage-windows-post-binding-proof.json`.
Native compilation and PowerShell parsing pass. Installed Windows PowerShell 5.1
performs the separately approved elevated operations.

A later cleanup guard records successful configuration creation before deleting
the generated file. It must not delete a pre-existing file when create-once
fails. A non-elevated test executes the actual parsed creation/cleanup code:
failed creation preserves existing bytes, while successful creation removes
only its owned file. The first 40-assertion run,
`4cb1d4838cce432aae1178ac0dd453bf`, predates this cleanup guard.
An initial final-rerun request was canceled at elevation and created no resources.
After the user explicitly requested another attempt, the current harness passed
all 40 native assertions with the guard enabled. All three current source hashes
and exact resource removal are independently verified. This closes that
current-harness validation gap, not the broader production gates below.

**Limits:** generated NTFS images and retained owner context, not a complete
backing graph, general cross-volume separation, physical-disk clone resistance,
nonprivileged collection, Linux continuity or production state admission.
The copied-image observation is diagnostic data, not retained live I/O authority.
SQLite startup, existing state/configuration and runtime dependencies remain
unchanged. The full persistent local-provider and bootstrap gates remain open.

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

## Explicit fresh setup

The user confirmed that the application is not released and has no existing
installation. Issue #68 now requires explicit fresh setup, not an existing-state
migration feature. Do not add a binding conversion or compatibility layer for
disposable development state.

Use new empty state for the new storage contract. Jobs, plans, settings,
approvals and receipts do not transfer from old state. Leave any pre-existing
state and media untouched. Report incompatible or partially initialized state
and require an explicit operator action. Never delete, reset, overwrite, approve
or rebind it automatically.

Establish the approved external state expectation before any SQLite creation,
coordination or recovery. A copied database or marker cannot supply it.
Existing configuration remains read-only. The authorized first-launch starter
path can create configuration once, but must not replace an existing file after
failure or reinterpret old records as new authority.

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

### Native Windows experiment

```powershell
.\scripts\storage-windows-prototype.ps1
```

This separate experiment compiles `storage-windows-prototype.cs` with the
installed PowerShell `Add-Type`. It requires no dependency installation and
changes no application crate, runtime binding, service, share, drive mapping,
or ACL. The C# collector is test tooling, not another production policy
implementation. A future Rust adapter must use the existing retained native
handles and reviewed platform I/O boundary, not invoke PowerShell or C#.

The harness creates only uniquely named temporary directories and one generated
junction. It retains directory handles during metadata checks, disposes them,
removes the junction without following it, and deletes its generated fixture.
The JSON output records assertions and source hashes, not personal paths.

The collector opens the drive or volume root once. Every later component uses
`NtCreateFile` with the actual retained parent in `OBJECT_ATTRIBUTES.RootDirectory`,
`FILE_DIRECTORY_FILE`, and `FILE_OPEN_REPARSE_POINT`. It rejects the actual
handle's reparse attribute rather than following a junction. It opens existing
directories only. A child walk uses the same parent-relative operation.
It never checks a full child path and then opens that path.

All identity reads require `FILE_ID_INFO`, including the full 128-bit file ID.
There is no short-ID fallback. Handle-relative device information checks
`FILE_REMOTE_DEVICE` before child traversal, including for mapped-drive anchors.
Remote or unavailable device evidence cannot fall back to local path spelling.

For a local NTFS handle, `GetFinalPathNameByHandleW` supplies a bounded normalized
volume GUID coordinate. The same volume GUID is the transient coordinate
namespace, not a new persisted identity. Compare whole components using
conservative ASCII case-insensitive rules, not drive letters or text prefixes.
Equal identities and nested coordinates overlap. Same-volume sibling coordinates
can be separated. Missing/short identities, remote evidence, invalid/Unicode
coordinates, different volumes, and unverified filesystems remain unknown.

The real fixture opens the same directory through both its drive path and volume
GUID path, and requires equal full identities and coordinates. It also covers
case aliases, nesting in both directions, sibling prefix lookalikes, actual root
and intermediate junction rejection, a junction reached from a retained parent,
missing roots, parent escapes and alternate streams. A moved root keeps its old
identity through its retained handle; reopening the original name after directory
replacement and marker copying must reject the saved evidence.

Reinspection compares complete component identities and the retained/new
coordinates, filesystem and remote flags. Windows can deny ordinary ancestor
renames while descendant handles remain open. An attempted generated ancestor
move was denied; that attempt is not counted as a successful ancestor-replacement
test. The existing Rust tests remain the separate evidence for ancestor behavior.

The result is `restricted_windows_replacement_passed`, not general Windows or
SMB acceptance. ReFS, different physical volumes, non-ASCII coordinates, Windows
containers, reboot, concurrent production components and power loss are not
newly verified. Coordinates can change during external relocation; retaining a
handle is not an atomic topology transaction. Require cooperative topology
administration and fresh reinspection before effects.

Microsoft documents [parent-relative NtCreateFile and no-reparse options](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile),
[full file identity](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info),
and [normalized handle paths](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew).
NTFS hard links apply to files, not directories; see
[hard links and junctions](https://learn.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions).
This proves directory-root relationships, not that file bytes have no other
hard-linked names. Keep the separate per-file ownership and mutation protections.

### Network-share evidence still required

Client-visible UNC names, mapped letters, share names, DFS aliases, filesystem
type strings, and remote volume serials do not establish physical separation.
Microsoft states that [SMB volume serial numbers need not be unique](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/bf691378-c34e-4a13-976e-404ea1a87738).
The full Windows file-ID comparison is documented for one computer, not as a
universal identity across arbitrary servers/exports.

Network shares have no volume GUID paths. Normalized SMB path lookup can also
fail when a parent is not accessible. Neither failure permits a weaker UNC,
leaf-ID, copied-marker, or server-name fallback. The generated remote-evidence
tests deliberately return unknown even for matching IDs and a claimed NTFS
filesystem. They are model checks, not live SMB tests.

The next isolated network fixture must expose generated backing directories
through sibling, equal, and nested exports, including different names for the
same data. It must establish which server/volume evidence actually survives
independent clients, reconnect and recreation. It must cover denied parent
access, offline evidence, server/export changes, unavailable full identities,
and local private storage also exported as media. DFS/referrals need an explicit
supported or rejected result. An export witness must bind to actual retained
native authority; an operator path or an export manifest alone cannot replace it.

The user approved Docker Desktop startup, Debian Samba/client package
installation in a disposable container image, and isolated CIFS mounts.
The kernel initially had CIFS built as an inactive loadable module. The user
then separately approved automatic loading inside the Docker Linux VM.
The approved mount succeeded and CIFS is now registered. No driver was installed,
Windows share created, host port published, or existing library accessed.

### Generated SMB export counterexample

```powershell
.\scripts\storage-smb-prototype.ps1 -AllowPackageDownload
```

For the separately approved retained-client-handle experiment:

```powershell
.\scripts\storage-smb-prototype.ps1 -AllowPackageDownload -AllowCifsMount -AllowAutomaticCifsModuleLoad
```

This command **requires separate package-download approval**. The switch is an
explicit acknowledgement, not permission inferred from running ordinary tests.
It selects an already installed SDK matching `rust-toolchain.toml`, installs
Debian's `samba`, `smbclient` and `cifs-utils` only inside a disposable container,
and creates a temporary image from that stopped container. No base image is
pulled and no BuildKit cache is created. It removes the exact generated
containers, internal network, volume and derived image afterward. Docker Desktop
remains running after its separately approved startup.

The package-installation container uses outbound network access. All later
servers and clients use an internal Docker network, with no published host ports.
The harness verifies this isolation. Containers have 512 MiB memory, one CPU and
128-process limits. The actual server/client filesystems and fixture mounts are
read-only; bounded tmpfs mounts hold runtime state. The server retains only its
required `SETUID` and `SETGID` capabilities. Clients have no capabilities.
Mounted-client mode adds only `SYS_ADMIN` in the client's own mount namespace,
with separate approval. No privileged mode, host mount namespace, host-directory
mount, or disabled seccomp profile is used. The CIFS module can remain registered
in the Docker VM after the generated lab is removed; the harness does not unload
a potentially shared driver.

`storage-smb-lab.conf` exposes generated directories through `media`,
`media-alias`, `staging`, and `private-alias`. Guest access is deliberately
read-only and confined to this isolated fixture, not an application configuration
or production recommendation. The first two exports share the exact backing
directory. The staging export is physically nested beneath it. A separate
private directory is also exported to demonstrate why remote-looking paths
cannot automatically be considered independent from local private storage.

The pinned Rust collector verifies the server's actual retained backing handles:
the staging/media pair overlaps, and the private/media roots are siblings. Two
coexisting SMB clients query each export and read generated control bytes.
Matching bytes are fixture controls, not authority for a real library.

**The measured counterexample reports different SMB volume serial numbers for
`media` and `media-alias`, even though they export the same directory.** These
values remain stable for each unchanged export across both clients and server
recreation. Thus stability and different serial values do not prove physical
separation. These are the `smbclient volume` values, not proof of full Windows
`FILE_ID_INFO` or a CIFS native device/inode binding.

The harness stops the server and requires a failed query. It then recreates the
server, verifies unchanged native backing identities, and requires successful
client reconnect and control reads. An offline error is not a confirmed missing
directory. No application index or binding row is modified by these checks.

The JSON result is `smb_export_counterexample_measured`. It records the SDK and
Samba versions, source hashes, reported serials, control metadata, backing
relationships, offline error and isolation checks. The measured packages report
Samba 4.17.12-Debian and Rust 1.99.1.

**Real mounted-client rejection checks now run.** The mounted mode executes the pinned
collector against four read-only CIFS exports on two independent clients and
a fresh client after server recreation.

The mount uses SMB 3.1.1, `nosuid,nodev,noexec`, and `serverino`. It resolves only
the internal fixture server. The standard mount helper requested capabilities
outside the approved client set; the harness instead uses the kernel's supported
direct mount path, retaining `SYS_ADMIN` only. See the
[Linux CIFS usage guide](https://docs.kernel.org/admin-guide/cifs/usage.html):
the helper is recommended but not required. Anonymous access and empty
credentials remain isolated fixture choices, not production guidance.

Each collector opens directory components without following links and retains
actual CIFS handles. The known nested exports and private-local/private-export
alias receive `UnknownFilesystem`, never `Separated`. Control reads through the
mounts confirm the generated aliases. This is actual native-client rejection
evidence, not a text-only policy simulation.

**The mounted experiment also reproduces unstable Unix binding identity.**
The same unchanged remote root has different `st_dev` values on the two clients
and on the recreated client. The backing identities remain unchanged.
`serverino` does not make the device/inode pair a portable saved root identity.
Neither its inode alone nor the reported SMB volume serial is a safe substitute.
The boundary-relative representation cannot fix a changed root identity.

**Positive network binding admission remains blocked by design, not tooling.**
The native-handle counterexample and explicit unknown rejection are verified.
They do not establish a stable provider-scoped remote identity, safe cross-export
ancestry, a trusted exact backing-namespace witness, or failover/DFS behavior.
No denied-parent, export-remapping, Windows-native SMB or offline CIFS freshness
acceptance is claimed. A future network adapter must establish those properties
or return unknown. Network media remains a product requirement, not an approved
unsupported feature.

### Network identity candidate review

This is a source and protocol review, not another mounted experiment. It reviews
Samba 4.17.12, the version used by the generated lab, and upstream Linux 6.12 and
6.18. It does not verify the Docker VM's complete downstream kernel source.

**A filesystem object ID is not a generic replacement for the export serial.**
Samba's
[`create_volume_objectid`](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/smbd/smb2_trans2.c#L2077-L2084)
hashes the service name with `E_md4hash`. Its
[`SMB_FS_OBJECTID_INFORMATION` response](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/smbd/smb2_trans2.c#L2524-L2537)
uses that result. Thus two names for the same backing directory can have
different object IDs. Changing a share's backing path without changing its name
does not change this computation. The ID cannot establish backing-root continuity
or separation. This is an implementation finding, not a measured object-ID query
against the lab.

**The reviewed Linux query ioctl does not query the retained remote open.**
[`cifs_ioctl_query_info` in Linux 6.12](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/ioctl.c#L26-L69)
builds a path from the file's dentry and passes that path to the protocol adapter.
[`smb2_ioctl_query_info` in Linux 6.18](https://github.com/torvalds/linux/blob/v6.18/fs/smb/client/smb2ops.c#L1557-L1760)
constructs a new open, queries its compound file ID, and closes it. An ioctl
invoked on an existing descriptor is therefore not enough to establish that its
response belongs to the original remote handle. Do not use this interface as a
handle-bound witness or add a path-reopen fallback.

The SMB protocol itself can query an existing open:
[`SMB2 QUERY_INFO` locates that open through its file ID](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/abb48417-27e5-4a16-82e0-3e5981db97e8).
The rejection above concerns the reviewed Linux ioctl implementation, not every
SMB client implementation.

**The on-disk ID create context is a separate candidate, not full identity
acceptance.**
Microsoft's
[`SMB2_CREATE_QUERY_ON_DISK_ID` handling](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/9b045a20-e4e7-4747-a504-a72df2f46490)
obtains file and volume values from the object store, but specifies their
uniqueness as `SHOULD`, not `MUST`. Samba's
[`QFid` response](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/smbd/smb2_create.c#L1530-L1555)
writes the volume value with `SIVAL`, retaining only 32 bits of `st_ex_dev`.
Its
[default file-ID adapter](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/modules/vfs_default.c#L3377-L3393)
returns the inode on the share's base device, or combines truncated device and
inode fields on other devices. These values do not establish a portable,
untruncated native identity for arbitrary Samba exports. Do not treat this
context as a fallback for full Windows identity.

[`FileIdInformation`](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/e4185a8a-ed8d-4f98-ab55-ca34dc8916e6)
can return a 64-bit volume serial and 128-bit file ID. The protocol requires a
zero file ID when the filesystem does not support that identity. Such a response
must remain unsupported, not become a short-ID fallback. No positive native
Windows SMB or Samba acceptance for this query is claimed here.

**Full leaf identity still does not prove cross-export ancestry.**
The
[SMB normalized-name rules](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/d64e0451-64a2-425a-848e-52a7dddab7b9)
use the share's local path as the boundary and reject names outside that boundary.
Samba's normalized-name implementation likewise uses the share-relative name.
Successful queries do not establish a common physical coordinate system for
different exports. Comparing these names would repeat the rejected
boundary-relative separation design.

The remaining design needs both provider-scoped full identity and a fresh,
handle-correlated relationship witness. Server-side native metadata is a possible
research direction, not an approved adapter or deployment requirement. Do not
add a daemon, endpoint, witness mount, or operator-manifest substitute without
separate design approval. Keep network media in scope and runtime integration
blocked.

### Server-side witness design candidate

The user approved investigation of a read-only server-side metadata witness.
This approval covers design only. It does not approve a server installation,
SMB extension, new endpoint, witness mount, application dependency, or runtime
binding change. This candidate has not passed an isolated implementation proof.
The retained-open association now has the restricted evidence below; the complete
identity, relationship and operation contract remains unproved.

**Purpose:** obtain physical relationship evidence from the storage server's
retained native handles, rather than infer it from client-visible export names.
The witness does not acquire media, change files, approve work, or move domain
policy to a server adapter. The API and worker stay on the same application host.
SQLite stays on local disk.

#### Required authority bridge

The candidate needs a verified association between the client's existing remote
open and the server's actual native handle for that same open. A server must
resolve it within the authenticated session and open table. Client-supplied
file-ID text, an export name, matching control bytes, or an operator manifest is
not this association. A new server open of the supplied path is not a substitute.

The server also needs explicitly registered native roots for the permitted
roles. Registration remains separate from passive inspection. Open and retain
every component through the existing no-follow primitives. Compare the remote
open's full native identity with the registered root's full native identity
before producing evidence. Recheck the registered ancestry and topology with
the retained handles. Reject replaced roots and reparse or mount crossings
outside the proved subset.

An authenticated witness identifies the evidence issuer, not the filesystem.
Each supported provider still needs a full native identity contract and measured
continuity across the claimed lifecycle. Unix `st_dev` does not become durable
merely because the server signs it. Windows identity still requires the full
high-resolution value. No short-ID, copied-marker or share-name fallback is
permitted.

#### Bounded request and response

The following is a logical contract for review, not a new Rust or JSON schema.

| Item                      | Required meaning                                                                                                                                                                                          |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Request scope             | Exact active configuration revision, root-binding versions and complete role inventory used by the admission decision. Native server paths come only from operator configuration, not request strings.    |
| Freshness challenge       | A fresh client challenge bound to that request and authenticated connection. A response from an earlier request, connection or server instance cannot be reused.                                          |
| Remote-open association   | The authenticated session's existing open, correlated by the server with the actual retained native handle. Missing, closed or mismatched associations fail.                                              |
| Native evidence           | Full root identities, supported backing-namespace evidence, retained ancestry and the result for each requested relationship. Never report a client path as a physical coordinate.                        |
| Live ownership            | A bounded server context that retains the native handles. Its validity ends on disconnect, expiry, restart or evidence change. A serialized response cannot replace these handles.                        |
| Result                    | Explicit overlapping, separated or unknown relationships, plus typed failures. Missing pairs, truncated metadata or unsupported providers cannot mean separated.                                          |
| Privacy and authorization | Return only authorized role metadata. Exclude media contents, private paths and credentials. Scope open lookup to the authenticated requester; never expose arbitrary server handles or root enumeration. |

Use the existing prototype bounds as the initial proof limits: at most 32 roots,
256 opened components, and the existing Linux mount-snapshot bounds. Keep context
counts, response sizes and lifetimes bounded before implementation. Their final
values and transport remain review decisions. Do not install a general
filesystem RPC service to bypass these decisions.

The transport must authenticate the configured issuer and protect the response.
Do not trust DNS names, SMB server GUIDs or echoed issuer fields alone. Reuse an
existing authenticated transport only after proving that it binds the response
to the exact open and requester. No new public MCP endpoint or anonymous remote
mode is allowed. Keep configuration inspection passive and secrets out of
serializable DTOs.

#### Relationship decisions

For roots within one proved native authority, the server applies the same Rust
relationship rule to actual handle-correlated evidence. Initially investigate
only the already proved same-volume NTFS and same-device ext4 subsets. A root
backed by another network filesystem is not automatically a local native root.
Hidden server-side bind aliases and descendant mounts retain their existing
unknown or rejection rules.

Different issuer identities do not prove different physical storage. A server
can export the client's private local root, or multiple servers can expose a
common backing filesystem. Comparing issuer names or public keys cannot repair
this. Cross-provider and client-local/server-remote pairs stay unknown unless a
separate verified backing-namespace bridge establishes the relationship.

Every required root pair must have supported evidence before new admission.
An unknown pair prevents that admission. This design therefore does not yet
settle private-local/remote-media separation, different-volume admission, or
arbitrary NAS support. Keep those requirements visible rather than reducing
product scope.

#### Admission is not file-operation authority

A read-only witness can help decide a root relationship. It cannot make a later
path-based SMB operation safe. A signed snapshot or a live evidence context is
not proof that a different open, rename or write uses the witnessed parent.

The client and server I/O adapters must separately prove that every component and
effect remains attached to the admitted native authority without following
links. Recheck new opens and effects at their own safe boundaries. Retain actual
operation handles and staging ownership through completion. The reviewed Linux
path-reopening query ioctl cannot provide this association.

If an implementation needs an enforcement hook rather than a read-only witness,
report that distinction for separate scope review. Do not silently turn this
metadata design into a media proxy, storage daemon or remote worker.

Keep filesystem and network I/O outside database transactions. Recheck the exact
configuration, role inventory, binding versions, claims and approvals when
committing bounded observations and before new effects. Expired or unavailable
evidence blocks new effects. It does not erase immutable plans, submitted-work
receipts or recovery evidence, and does not confirm absence.

#### First isolated proof and stop conditions

The recommended first proof target is a Samba-side native-open hook in the
generated lab, paired with a client that can expose and retain its exact SMB
open. Samba already passes an open's `files_struct` to its VFS filesystem-control
hook; see the
[reviewed default VFS adapter](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/modules/vfs_default.c).
This supplies a source-level investigation point, not proof that a complete
extension or client bridge exists.

The current anonymous generated fixture can test handle correlation, but cannot
establish authenticated issuer or protected-transport acceptance. Those checks
need separately designed generated identities, not live credentials.

The first proof must compare the same open's native identity and relationships
with the independent native collector. Test equal, nested and sibling exports,
aliases of private roots, retained opens during replacement, export remapping,
missing identities, foreign or closed opens, disconnected sessions, stale
responses, changed inventories, and server/client recreation. Keep unsupported
cross-provider pairs unknown. Demonstrate that changing an export does not
retarget an existing open's witness.

Stop if the client cannot identify its retained remote open, the server hook
reopens a path, the provider lacks full identity, or the relationship evidence is
incomplete. Do not add weaker fallbacks. Parent-relative operation safety needs
its own race and traversal proof before production use.

Implementation of this proof needs separate approval. Any additional package
installation, server extension, transport exposure or host change also needs its
own approval. Existing package and mount approvals do not authorize these new
operations. No positive network admission or runtime integration follows from
this design document alone.

### Measured retained SMB open bridge

The user directed continued proof work and separately approved additional client
tools inside the disposable Docker lab, with Astral `uv` wherever possible:

```powershell
.\scripts\storage-smb-prototype.ps1 -AllowPackageDownload -AllowWitnessTools
```

The combined run also retains the separately approved CIFS checks:

```powershell
.\scripts\storage-smb-prototype.ps1 -AllowPackageDownload -AllowWitnessTools -AllowCifsMount -AllowAutomaticCifsModuleLoad
```

`-AllowWitnessTools` acknowledges the additional installation approval. It
installs `uv` 0.13.8 from its exact SHA-256-verified Linux wheel and uses
`uv venv` and `uv pip install` for `smbprotocol==1.17.0` and its dependencies.
It uses the container's existing Python with managed downloads disabled. Nothing
is installed on the host. No new server extension or public port is added.

**The existing Samba hook supplies a real retained-open association.** In the
reviewed 4.17.12 implementation, `FSCTL_CREATE_OR_GET_OBJECT_ID` returns the
open's complete native device and inode fields through `push_file_id_16`.
The [native field encoding](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/lib/file_id.c#L45-L59)
preserves both 64-bit values. The client sends the exact existing SMB file ID,
not a pathname or the path-reopening Linux ioctl. The server's existing VFS hook
uses the open's `files_struct`.

The control operation's name permits creation on other servers. It is not a
generic read-only query and must never be used as such. This experiment checks
the exact reviewed Samba version before starting its client. The source returns
computed fields without creating metadata. Actual directory timestamps and
extended attributes remain unchanged across the initial queries.

Two independent clients match the server's retained no-follow native directory
handles and the independent Rust collector. The same-directory exports agree on
the full native identity despite their different export object IDs. After a
generated root is replaced with a copied control file, the original SMB open
still returns the original native identity. A fresh connection identifies the
replacement. Native ancestry reinspection rejects the replacement. Foreign-tree
and closed-open requests fail with `STATUS_FILE_CLOSED`. The harness restores the
original generated directory and closes its contexts.

The native server context also reads a nonzero 16-byte filesystem UUID from its
retained directory handle with
[`FS_IOC_GETFSUUID`](https://github.com/torvalds/linux/blob/v6.18/include/uapi/linux/fs.h#L325-L336).
It neither creates an ID nor reads a marker. The full native identities and UUID
remain unchanged after server recreation, when the entire bridge and replacement
experiment runs again. This is server/container recreation evidence, not a
server reboot, filesystem-clone or different-provider identity proof.

**This remains a fixture bridge, not a deployed witness.** Docker's supervised
control channel supplies the native observations independently. Ordinary SMB
does not return this experiment's native UUID or complete physical ancestry.
The generated guest client explicitly disables signing, encryption and secure
negotiate verification; no authentication acceptance is claimed. These settings
exist only in this fixed internal fixture, never in application startup or a
production fallback.

The result records `retained_samba_native_identity_bridge_measured`, both server
generations, source hashes, native UUIDs, unchanged metadata, replacement and
invalid-open outcomes. The final combined run also passes the prior two-client
and recreated-client CIFS unknown-rejection checks. Exact generated containers,
network, volume and image are removed.

A complete provider still needs an authenticated way to deliver native namespace
and ancestry evidence from the same retained server authority. UUIDs alone do
not establish root continuity or global physical separation. Parent-relative
operation safety remains a separate gate. Do not persist this diagnostic as
root authority or deploy the guest client.

### Measured CIFS parent replacement

The combined witness/CIFS command also tests actual descriptor-relative child
access after a generated server directory is replaced. It requires both
`-AllowWitnessTools` and `-AllowCifsMount`; it adds no packages, capabilities,
host mounts or exposed ports beyond the separately approved lab.

The client opens each directory component without following links and retains
the parent descriptor. It reads a bounded generated `sentinel` through
`os.open(..., dir_fd=parent)`, recording both parent and child inode values.
The server then renames the original parent and places a different generated
parent at its former name. The client repeats the child open through the same
retained descriptor, not through a newly opened parent path.

**The actual replacement test returns `ESTALE` (errno 116).** Its result is
`cifs_parent_replacement_rejected`. The original marker and inode values were
read before replacement. The later child open failed; no replacement marker or
child identity was returned. The harness reports this refusal explicitly and
restores and removes only its generated directories and marker files.

This is safe refusal in one controlled replacement case, not evidence of
wrong-object access. It also does not prove that every CIFS child operation uses
the original server-side native parent. The test does not exercise a concurrent
change-and-restore race, writes, rename effects, DFS, failover or Windows SMB.
Other expected filesystem refusals remain explicit diagnostic errors;
unexpected errors fail the experiment.

The full run repeats the retained SMB/native bridge across server recreation
and preserves the mounted alias rejection checks. Its saved source hashes match
the exercised Python and PowerShell sources. The anonymous fixture still does
not establish a production provider or authenticated ancestry transport.

The user subsequently approved the isolated read-only server-hook proof below.
It supplies restricted native-parent evidence, not a complete network provider.
Do not replace the remaining proof gaps with repeated path checks.

### Server-side operation safety design

The user first selected design-only investigation of server-side operation
safety, then separately approved the isolated read-only implementation below.
Its boundary is one hook inside the generated Samba server, not a new daemon,
media proxy, public endpoint or application worker.

#### Verified interface constraints

The ordinary
[SMB2 CREATE request](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/e8fb45c1-a03d-44ca-b7ae-47385cfd7997)
names a file relative to its share, not to a client-selected existing directory
open. Its packet has no parent file-ID field. The
[SMB2 rename information](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/52aa0b70-8094-4971-862d-79793f41e6a8)
requires `RootDirectory` to be zero for network operations. These interfaces do
not provide the required native-parent association by themselves. This is not a
claim that every provider extension or native SMB client is unsafe.

The reviewed Samba 4.17.12 source provides narrower implementation points:

- [Filesystem-control dispatch](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/smbd/smb2_ioctl_filesys.c#L781-L807)
  passes the request's resolved existing `files_struct` to `SMB_VFS_FSCTL`.
- [Native VFS operations](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/modules/vfs_default.c)
  include descriptor-relative `openat`, handle-based reads and metadata, and
  descriptor-relative `renameat`. The rename implementation uses ordinary
  `renameat`, not a no-replace primitive.
- [Normal SMB creation](https://github.com/samba-team/samba/blob/samba-4.17.12/source3/smbd/smb2_create.c#L973-L1053)
  converts the request filename to a parent before calling the VFS create
  operation. This parent is not automatically the client's separately retained
  directory open.

These interface findings alone do not prove an extension. The presence of
`openat` inside a server is not proof that a request uses the intended parent.

#### Smallest isolated proof

Propose one provider-specific filesystem-control request on an existing SMB
directory open. Use a separately assigned private operation code; never
reinterpret a standard control code or extend the object-ID query with effects.
The hook borrows that request's native parent for the full operation. It must
not reconstruct the parent from `fsp_name`, an export path or a client string.

The first proof accepts only one generated child component, one fresh challenge
and a supported protocol version. Bound the input to 4 KiB, output to 1 KiB,
child name to 255 ASCII bytes, and marker read to 32 bytes. Reject separators,
empty names, dot components, alternate-stream syntax and invalid lengths.
These are proposed diagnostic limits, not new application filename policy.

Reuse the witness contract's configuration revision, binding versions and
complete role inventory. Match the actual parent to its explicitly registered
role before opening the child. A valid but unrelated SMB directory open is not
an allowed parent.

Open the child without following links relative to the retained native parent.
Use a nonblocking open and reject non-regular objects before reading bytes.
Require the actual child to be a regular file in the proved mount/filesystem
subset. Read identity, permissions and the generated marker from that same
child handle. Return the challenge, complete parent/child native identities and
explicit success or refusal. Do not return arbitrary file contents or private
paths. Close only handles the hook owns; never close Samba's borrowed handle.

The hook must preserve the requesting session's access rights and server share
policy. Knowing a directory file ID does not grant access to every child.
Check the existing-open association and required access explicitly. Do not
become root, bypass ACLs or inherit a test guest exemption into production.
The current guest lab can prove mechanism only. Generated authenticated sessions
and protected, configured-issuer transport need their own acceptance.

Run a single request at a time in the first proof. Add no durable state or
cross-request handle table. Retain the native parent and child until completion,
including cancellation and asynchronous close. Refuse unavailable native
handles, unsupported VFS stacks, short identities and mount crossings.
Native topology must remain cooperative, as in the existing local proof.
Recheck configured binding evidence before publishing a result; retained handles
do not make external directory relocation an atomic transaction.

#### Acceptance and failure cases

Compare exact parent and child identities with the independent native collector.
Use generated equal, nested and private aliases. Replace or rename the parent
and child at controlled pauses. Each response must refer to the original
permitted object or report refusal, never silently switch to replacement data.
Also test intermediate and leaf links, bind mounts, inaccessible children,
foreign sessions/trees, closed parents, disconnect, stale challenges, oversized
messages and unsupported versions. Recreate the server and require old contexts
to fail. Verify bounded resource use, unchanged fixture metadata and exact
cleanup. Source inspection alone cannot pass these cases.

#### Later effects remain separate

The first hook does not issue a reusable SMB child open and does not support
audio streaming, creation, writing, rename, deletion or recovery. A raw native
descriptor cannot simply be advertised as an SMB file ID. A later design must
preserve Samba's access checks, handle lifetime, share modes and close behavior,
or explicitly review another bounded transport. Do not silently expand this
diagnostic hook into a media RPC service.

Future effects must retain the same parent/file authority, preserve staging
ownership and recheck application claims, configuration and approvals before
submission. Keep business policy and SQLite in the application; the server
boundary enforces native I/O authority, not a second approval implementation.
Publication needs a true native no-replace operation, not ordinary `renameat`
or an existence check followed by rename.

A lost response after submission is an uncertain effect, not permission to
retry. The existing SQLite intent and recovery rules remain authoritative.
Do not promise exactly-once remote effects or add a second server database.
Durable reconciliation, exclusive object ownership, file/directory sync and
crash recovery must be proved before writes. If the selected interface cannot
provide them, refuse that effect and review the gap.

#### Build and deployment gate

The proof needs matching Samba source/internal headers and an isolated module
build. The installed development package alone does not provide the reviewed
internal open structures. Source, build packages, module loading and any license
or packaging implications need review and separate approval. Keep Samba
4.17.12 as the existing fixed fixture, not a recommended production version.

An extension-dependent design would exclude servers that cannot load it.
That is an unresolved deployment constraint, not approval to drop general
network media or claim arbitrary NAS support. Client-local/remote relationships,
authenticated ancestry delivery and Windows SMB remain separate proof gaps.
Do not start production integration from this design.

### Measured read-only native server hook

The user separately approved matching Samba source, build packages and loading
one test module inside the disposable lab. This approval does not cover host
installation, existing shares, media writes or production integration.

```powershell
.\scripts\storage-smb-prototype.ps1 -AllowPackageDownload -AllowWitnessTools -AllowServerHook -AllowCifsMount -AllowAutomaticCifsModuleLoad
```

`storage-smb-native-probe.c` is an original diagnostic module, licensed
GPL-3.0-or-later. `storage-smb-hook-build.sh` verifies the official Samba 4.17.12
archive SHA-256 and release signature on download. The recorded signing
fingerprint is `81F5E2832BD2545A1897B713AA99442FB680B620`.
Builds use one CPU, 512 MiB memory and warnings denied. This is a fixed
experimental version, not a production recommendation.

**The server and module must use the same build.** The first loaded module
linked to source-built private libraries while the lab started Debian's server.
Share connection failed with a module registration collision. The lab did not
bypass this failure with a library-path override. It now builds and starts
`smbd` from the same verified source as the module, checks both versions and
missing dependencies, and records source, module and server hashes. Debian's
client package version remains separate from the actual hook server version.
No compatibility with Debian's installed `smbd` is claimed.

The private control `0x00096404` acts on the request's existing SMB directory
open. The hook borrows `fsp_get_pathref_fd(fsp)` for the child open. It registers
and retains no-follow native root chains for the four fixed fixture roles.
Path reinspection checks that binding; it never supplies a replacement parent
for the child operation. Ordinary controls pass through unchanged.

The request has a fixed fixture revision, fresh challenge, strictly increasing
per-tree sequence and optional observed full child identity. Input is at most
4 KiB, the name at most 255 ASCII bytes, and the response at most 128 bytes.
Marker reads are at most 32 bytes. Invalid names, versions, lengths, revisions
and replayed sequences are rejected. The decoder also rejects responses with
the wrong challenge, revision, sequence, identity or result shape. This fixed
revision is not production settings or binding-version acceptance.

**Both server generations pass the actual read-only experiment.** Exact native
parent identities match the independent collector for all four exports.
Successful child identities match its retained-handle observations. Initial
queries leave measured directory timestamps and extended attributes unchanged.
All 14 malformed or replayed requests fail, and a subsequent valid request
succeeds. Metadata-only opens cannot authorize child reads. An unrelated parent
is refused. Actual inaccessible, symbolic-link, FIFO and oversized-file cases
return `EACCES`, `ELOOP`, `EINVAL` and `EMSGSIZE`, with no marker bytes.

During a controlled parent-replacement pause, the borrowed parent opens the
original child. The final binding check returns `ESTALE`, with the original
parent/child diagnostic identities and no bytes. A fresh context identifies and
reads the replacement fixture. During child replacement, the expected-child
identity check returns `ESTALE` before reading replacement bytes. The harness
restores the original parent and child and verifies their identity and marker.
Foreign-tree and closed-parent requests fail.

A separate server exposes an actual child bind mount. The hook observes that
child's full native identity but rejects the different mount with `EXDEV`
(errno 18), returning no marker. The full run also preserves the earlier
retained-open bridge, CIFS unknown-rejection, parent-replacement refusal,
offline protocol failure and server recreation checks. Four decoder tests pass.

The result is `read_only_native_parent_hook_measured`, not network admission.
All seven source hashes match the saved experiment. Its exact containers,
internal network, generated volume and derived image are removed. The ten
separate source/build containers and seven cache images are also removed.
Docker Desktop and the previously approved shared CIFS module remain available.

**Remaining limits:** generated Linux/ext4 data, fixed role inventory and
anonymous fixture sessions only. The hook does not deliver authenticated
filesystem UUID/ancestry evidence, issue reusable child opens, stream media,
write, publish or recover files. Protected issuer/session transport,
intermediate-link and disconnect races, change-and-restore, failover/DFS,
Windows SMB and arbitrary NAS compatibility are not accepted. Resource limits
come from the fixture and Docker controls, not a production concurrency proof.
Native topology still requires cooperative administration.

The next gate is authenticated same-authority identity and ancestry delivery,
followed by the separate production binding-design review. Network media remains
required. A server-specific diagnostic is not approval to exclude other NAS
devices or integrate the hook into the application.

### Authenticated network evidence design

The user selected **encrypted SMB plus a pinned witness public key**.
The design selection alone did not approve new accounts, keys, packages or
module loading. The user subsequently approved the isolated authenticated proof
below. Neither approval permits production installation or runtime integration.

#### Transport and issuer

Use the existing SMB connection for the witness control and its directory opens.
Do not add a daemon, HTTP endpoint, bearer-token exchange or public MCP service.
Require SMB 3.1.1, successful non-guest authentication, preauthentication
integrity and encryption for every witness request and response. Validate the
actual negotiated state and received messages, not just connection options.
Never retry without encryption, signing protection or authenticated access.

[Microsoft's SMB security guide](https://learn.microsoft.com/en-us/windows-server/storage/file-server/smb-security)
describes encryption's privacy/integrity protection and SMB 3.1.1's protection
of negotiation and session setup. Encrypted messages have integrity protection;
they do not need a separate SMB signature as well. The
[signing guide](https://learn.microsoft.com/en-us/windows-server/storage/file-server/smb-signing)
recommends Kerberos over NTLMv2 and rejects disabling protection for guest access.
The reviewed
[smbprotocol 1.17.0 session implementation](https://github.com/jborean93/smbprotocol/blob/v1.17.0/src/smbprotocol/session.py)
also refuses guest/null sessions when encryption or signing is required.
These are documented properties and source findings, not an authenticated lab
result or approval of that Python package as the production Rust adapter.

Prefer Kerberos where the operator has a correctly configured service identity.
Do not require a Kerberos domain for this design. An explicitly selected
non-domain authentication profile still needs a reviewed NTLMv2 implementation,
encrypted transport and the pinned witness signature. Reject negotiation of
unapproved authentication mechanisms. Never infer trusted issuer identity from
DNS, a server GUID, an account display name or an echoed key.

The operator supplies the witness public key through read-only startup
configuration. The proposed signature is Ed25519, as specified in
[RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html#section-5.1).
Use a reviewed implementation, not hand-written cryptography.
The application accepts only the configured key and protocol profile.
Do not learn a key on first connection or trust a key returned in the response.
SMB account authentication authorizes the requester; the witness signature
identifies the configured evidence issuer. Neither replaces native filesystem
proof or protects against a compromised trusted server.

#### Exact signed meaning

Use a versioned, strictly bounded binary message with one canonical encoding.
The signed bytes have a fixed witness-purpose prefix and include the full
request digest, response metadata and result. The request contains a fresh
256-bit challenge, active application configuration revision, root-binding
versions, complete requested role inventory and a monotonically increasing
context sequence. Native paths never come from the request.

The server binds its response to its actual authenticated requester, SMB
session, tree and existing open IDs, not just copies supplied identifiers.
Include a fresh server-instance identifier, witness configuration revision,
registered-root inventory revision, policy/protocol version and live-context
identifier. The client compares both the signed fields and the actual received
SMB envelope with its pending request and existing opens. A signature over
reopened-path metadata or unrelated Docker observations is not accepted.

The live context retains every required native root/component handle. Each
root must match its registered role and the requester's allowed metadata scope.
Check native root binding and topology before and after collection.
Return full supported native identities, full filesystem UUIDs and the complete
required relationship matrix. Reject zero or truncated identity and incomplete
inventories. An unsupported pair remains unknown, never separated.

Keep private native coordinates inside the server. Relationship evaluation must
reuse the one authoritative Rust rule on native handle-correlated evidence;
do not copy that rule into C or trust share-name comparisons. Any shared-core
linkage and provider implementation need their own review. Signed replies return
authorized role identities, bounded ancestry identity evidence and relationship
results, not private paths, directory listings or media bytes.

The first proof proposes at most 32 roots, 256 retained components, 4 KiB request
and 64 KiB response, with no paging or truncation. Refuse evidence that exceeds
these bounds. Use one outstanding request per context, a five-second request
deadline and a maximum 30-second context lifetime, measured with local monotonic
clocks. No timestamp supplied by the other host extends those limits.
Disconnect, expiry, restart, changed registration or changed evidence ends the
context. A signed response remains an observation, not durable I/O authority.
These limits are proposed proof settings, not accepted application defaults.

The server cannot validate application SQLite state from echoed revisions.
The application must recheck its actual configuration, role inventory and
binding versions after verification and before committing observations or
new effects. Keep network and filesystem work outside database transactions.
Authentication does not make server `st_dev` persistent or a filesystem UUID
globally unique. Reboot/remount continuity, clones, different providers and
client-local/remote separation remain separate proof requirements.

#### Key and credential ownership

Keep the witness private key only on the evidence server, outside every export.
Load it through a protected no-follow handle during trusted startup.
Keep signing operations separate from the application's RSA verification
dependency. Do not reuse TLS, Entra or SMB session keys as witness signing keys.
Never serialize private keys or credentials into DTOs, SQLite, URLs or logs.
The witness has no second database and cannot approve application work.

Pin or key rotation is an explicit operator action with coordinated stop,
configuration validation, restart and fresh binding review. Invalidate live
contexts. Never silently accept a new key or rewrite existing plans, approvals
or submitted-work receipts. Saved observations need their original issuer and
revision so they remain historical evidence, not current permission.

#### Next isolated acceptance and stop gate

The next proposed lab needs two generated authenticated users and a generated
witness key. Server private keys and account state must use a separate generated
server-only secret volume. Never place them in the fixture volume or committed
image shared with clients. Each client receives only its own generated
credential and the public pin. Use no live accounts or credentials.

Keep the internal network, no host ports or host mounts, read-only server
filesystem and fixture data, and existing CPU/memory/process limits. The first
authentication proof is protocol-only and needs no client `SYS_ADMIN`.
Test real encrypted controls from both users, denied roles, guest/null refusal,
bad credentials, unapproved mechanisms and missing transport protection.
Test wrong pins, changed signed fields, forged signatures, wrong challenges,
foreign sessions/trees/opens, replay, expiry and restart. Require valid requests
to continue after each refusal. Compare signed UUID, ancestry and relationships
with independent native observations, including hidden aliases and replacement.

The first proof must verify that client containers cannot obtain the server
private key. An external test signer or a key embedded in the shared client
image cannot pass issuer isolation. Key loss or signature failure must stop
new admission, not trigger a guest or path-based fallback.

The user subsequently approved the generated users, secret volumes, signing
implementation, matching source/build and module run inside the disposable lab.
The measured result follows. General NAS support remains unresolved.
Complete the production binding-design review before runtime integration;
streaming, writes, publication and recovery remain later gates.

### Measured authenticated native witness

The isolated authenticated proof passes. It uses the matching source-built
Samba 4.17.12 server and module, Rust 1.99.1, smbprotocol 1.17.0,
pyspnego 0.12.4 and cryptography 50.0.2. Approved tools are installed inside
generated containers with uv. No application dependency or production behavior
changes.

The C module signs with GnuTLS Ed25519. A linked Rust bridge reuses the existing
relationship rule. It duplicates borrowed descriptors, checks full native
identity and descriptor mount metadata, and requires an unchanged bounded
mountinfo snapshot. Native physical coordinates stay inside the server.
The module uses Samba's actual SMB2 tree encryption requirement. The reviewed
SMB2 dispatcher refuses plaintext before VFS dispatch; the SMB1-only
`encrypted_tid` field is not used as SMB2 evidence.

The fixed request is 148 bytes. The response has an 840-byte canonical body
and a 64-byte signature. It binds the complete request, fresh challenge,
generated frozen configuration/inventory hashes, actual Unix requester, SMB
session, tree and existing open IDs, signer instance and live native context.
Four fixed roles have full native identities, 16-byte filesystem UUIDs,
complete bounded native component ancestry and a 4-by-4 relationship matrix.
Unrequested records are zero; unrequested pairs are explicitly unavailable.
No native pathname, directory listing or media byte is returned.

Both generated users pass before and after server recreation and after explicit
key rotation. Each decoded response matches an independent native collector's
identity, UUID and complete ancestry. Its relationship matrix matches the
standalone Rust collector. Ordinary requests leave the measured native
modification/change timestamps and extended attributes unchanged.

The measured negative cases include:

- Each client rejects nineteen altered, malformed, replayed or foreign decoder
  and server inputs. Changed nonzero configuration/inventory hashes are refused.
  Seven standalone decoder cases also check correctly signed malformed shapes
  and a wrong actual server configuration hash.
- Actual bad-password, guest-account, empty-account, NTLMv1 and older-dialect
  exchanges are refused. The NTLMv1 test observes an actual 24-byte response
  on the wire. It does not count a pre-wire client failure as server evidence.
- An actual correctly signed plaintext witness request is refused. Encrypted
  sends and successfully authenticated decrypts are observed for valid controls.
- Foreign open, tree and session contexts, closed opens, disconnected contexts,
  and contexts from a previous server generation are refused on actual requests.
  Valid requests continue after malformed-input and transport refusals.
- Thirty-second contexts expire. A fresh tree/open obtains a new working
  context. Changed native bindings permanently invalidate the old context,
  including after exact restoration of the original directory.
- Controlled root replacement returns `NT_STATUS_FILE_INVALID` and no native
  evidence. Independent native observations identify the replacement through
  a fresh context. Another fresh context identifies the restored original.
- A real Docker bind alias exposes staging as the private role. Signed evidence
  identifies the exact shared native object and reports overlap with media in
  both directions. Authentication never converts this hidden alias to separated.
- Explicit rotation uses distinct generated secret volumes and new operator
  pins. Old pins reject the new server's real signatures. Missing signing-key
  material prevents module activation and stops native evidence without a
  guest, unsigned or path-based fallback.

Private keys and account state are generated after the shared image is committed.
An unmounted image check finds no witness key or client credential. Each client
gets only its own read-only secret volume, and neither can access the server
key. Runtime checks verify read-only volume mounts, no host-directory mounts,
no published ports, an internal network, 512 MiB RAM, one CPU and 128 processes.
Clients have no added capability; the server adds only SETUID and SETGID.
Exact generated containers, volumes, network and derived image are removed.

The Rust bridge passes ten tests, including descriptor ownership and unchanged
output on stale-identity failure. The earlier protocol decoder passes four tests.
Session evidence records the exact ten source hashes. Earlier read-only evidence
remains historical evidence for its earlier source revision.

This is a fixed four-role Linux/ext4 diagnostic, not a production provider.
The signed configuration field hashes the actual `smb.conf` file, not every
effective command-line override. Application/inventory hashes are generated
fixture revisions frozen within a context, not authoritative SQLite acceptance.
The signer-instance identifier is transient and process-scoped, not durable
server identity. Restart tests use actual new connections; they do not prove
recovery of an in-flight production command.

Authentication does not make native device numbers durable or filesystem UUIDs
globally unique. Different providers, client-local/remote separation, clones,
reboots/remounts, DFS/failover, Windows SMB and general NAS compatibility remain
open. The diagnostic opens do not register durable application bindings or
approve media effects. Production integration still needs the binding-design
review. Streaming, reusable child handles, writes, publication and recovery
remain separate gates.

### Measured bounded read-only operations

The user approved extending the generated Samba lab to directory listing,
metadata and complete file reads of at most 1 MiB. The user separately approved
restoring the existing dependencies and matching source inside disposable
containers. This permits no Windows installation, real media writes, public
service or production integration.

The isolated extension passes on the matching source-built Samba 4.17.12 server
and module. This is Windows-host Linux/ext4 container evidence.

The extension is opt-in through `-AllowReadOnlyOperations`, together with the
existing explicit package, hook and authenticated-witness approval switches.
The server also requires `DOMUS_SMB_READ_PROOF=1`. Without this flag, the
additional operations are refused.

Operations use the same encrypted SMB connection, authenticated requester,
existing directory open, pinned signing key and frozen native context as the
witness. They use the actual borrowed native parent descriptor, not a parent
pathname. Each child is one bounded component opened relative to that parent
without following links. Metadata comes from the actual child handle. Mount
crossings, non-regular files, multiple hard links and oversized files are refused.

The diagnostic limits are 128 directory entries, 255 bytes per component,
4 KiB input, 49,152 payload bytes per response, 32,768 bytes per read range and
1 MiB per file. These are fixture limits, not product restrictions.
The existing five-second request and thirty-second context limits remain.
A child request accepts ASCII names only in this fixture. This does not define
the product's Unicode filename policy.
A directory response contains names, not an atomic catalogue. A changed
directory version during enumeration causes refusal.

Metadata binds device, inode, size and high-resolution modification/change
times. Every read range carries that complete expected version. The server
checks the actual opened handle before and after the read. A changed version
returns no bytes. The client assembles a complete file only from matching
ranges and checks every byte against independent generated content.

Ed25519 covers the returned bytes and a SHA-256 digest of the complete request,
including child name, expected metadata, offset and range length. Existing
challenge, requester, session, tree, open, inventory and native-context checks
remain. Refused operations return no payload or native records. They invalidate
the native context; explicit fresh contexts remain separate.

The generated cases include empty, partial-last-range, exact-limit and
over-limit files. They also cover directory bounds, denied reads, links, FIFOs,
changed observations, malformed names, invalid ranges, altered signed bytes,
actual parent/child replacement and a real native child mount crossing.

Run `631e444cca4147cb91c6a1d4f117a1ae` is recorded in private session evidence
`files/storage-smb-read-operations-first-proof.json`. Both users pass on the
original server, after recreation and after key rotation. These six client runs
verify 6,488,166 exact file bytes across 204 ranges. Each verifies three complete
files, sixteen native/wire refusals and two changed-signature/request refusals.
The exact 128-entry directory passes. A 129-entry directory returns no entries.
The actual child mount returns `EXDEV`, observed-child replacement returns
`ESTALE`, and parent replacement returns `NT_STATUS_FILE_INVALID`; none returns
file bytes.

The earlier authenticated witness checks still pass on this source, including
role limits, plaintext refusal, session/tree/open association, expired contexts,
native root replacement, actual hidden aliases, key rotation and private-key
isolation. Ten recorded source hashes match the final files. Generated
containers, volumes, network and derived image were removed. The Windows host
and application runtime were not changed.

This is not a reusable child-handle protocol, a general NAS provider or
production streaming acceptance. It adds no creation, writing, rename, delete,
publication or recovery operation. Durable provider scope, local/remote backing
authority and the separate production integration review remain open. No live
in-place-write or enumeration/mutation race was induced.

### Measured support and remaining gates

| Environment or case                                                               | Evidence                                                                                                                                                                                    | Current conclusion                                                                                   |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Native Windows generated same-volume NTFS directories                             | Real retained-handle collector, normalized drive/GUID aliases, child operations, junction refusal and replaced-root rejection                                                               | Restricted native Windows experiment passes. Not production or SMB acceptance.                       |
| Windows-host Linux Docker, generated local ext4 volume                            | Current prototype source, actual bind aliases, child walks, side-by-side containers and recreation                                                                                          | Restricted relationship experiment passes. No production API/worker integration.                     |
| Windows-host Docker, generated Samba/CIFS exports                                 | Real retained backing and client handles, equal/nested/private aliases, different reported serials, changed native device identities, protocol offline failure and server/client recreation | Network binding counterexamples and unknown rejection verified. Positive admission remains unproved. |
| Missing, changed, malformed, opaque, or different-filesystem evidence             | Model tests; real missing-root and procfs checks                                                                                                                                            | Explicit error or unknown. Never authorizes separation.                                              |
| New bind mount during a live operation                                            | Real existing child bind mount rejected; changed snapshot model rejected                                                                                                                    | An actual live mount/unmount race was not induced. Host topology must remain stable.                 |
| Network media, Docker host-file sharing, non-ASCII coordinates, other filesystems | No replacement admission proof                                                                                                                                                              | Requirements remain open. No blanket product ban and no invented support.                            |
| Native Ubuntu 24.04 Azure VM, generated ext4 SQLite fixtures                      | Both adapter modes and Rust ownership pass directly as an ordinary user; exact transferred sources and resource cleanup checked                                                             | Bounded SQLite proof passes. No provider/backing or production integration acceptance.               |
| Native Linux relationship/backing proofs, Linux-host Docker, Windows containers   | No replacement admission proof on these hosts                                                                                                                                               | Not verified.                                                                                        |
| Host/VM reboot, production simultaneous API/worker/CLI use, recovery              | No new evidence                                                                                                                                                                             | Later integration acceptance remains required.                                                       |

The next review can accept the separation of stable binding identity from
short-lived relationship evidence and the measured Linux/Windows subsets. It cannot
approve arbitrary layouts or close issue #68. Keep runtime integration blocked
until that separate review and explicit handling of the required deployment
layouts.
