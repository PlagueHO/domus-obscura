# Disposable-volume recovery lab

## Permission and evidence boundary

This lab extends the [process-crash checks](crash-recovery.md). It is not a
production command or an installed-service test.

Use fresh generated content on disposable Azure Windows and Ubuntu 24.04 x86_64
VMs. Run one VM at a time. The total task cost ceiling is **USD 10**, including
compute, Windows licensing, disks, networking, and evidence transfer.

Get separate approval before creating Azure resources, installing guest tools,
creating a non-administrator test account, changing permissions, or managing
virtual disks. Repository implementation or plan approval is not that approval.
Do not run volume tests on the workstation, an existing library, or shared disks.
Do not install services, reboot the guest, contact live providers, or change
public exposure.

The wrappers select private Rust test-host entry points. They do not create,
format, mount, detach, or delete disks. Run the application probes as a normal
user. Keep privileged operator commands in a separate session.

These are real filesystem-full and virtual-volume-unavailability tests. They
are not physical device removal, full host-volume, OS-crash, or power-loss proof.
Manual TLS reload tests do not prove managed production certificate activation.

## Azure execution gate

Before provisioning, record the exact subscription, region, image versions,
VM sizes, disk SKUs, network route, payload-transfer method, and cleanup targets.
Do not copy subscription details or credentials into public issue evidence.
Select images and prices at execution; do not assume current availability.

Use Azure CLI inspection before requesting resource approval:

```powershell
az version
az account list --query "[].{name:name,id:id,state:state}" --output table
az vm list-skus --subscription <approved-subscription> --location <approved-region> --resource-type virtualMachines --output table
az vm image list --location <approved-region> --publisher <selected-publisher> --offer <selected-offer> --sku <selected-sku> --all --output table
```

Use the [Retail Prices API](https://learn.microsoft.com/en-us/rest/api/cost-management/retail-prices/azure-retail-prices)
to price each selected consumption meter in USD. Follow pagination. Exclude Spot,
reservation, and savings-plan prices unless those purchase terms are explicitly
approved. Include Windows licensing and disk billing rules.

The initial approval covers two VMs total and one running VM at a time.
Get separate approval for each additional VM. Keep all retries within the same
USD 10 total ceiling. A failed setup does not extend an existing deadline or
approve a replacement VM.
Use only task-owned resource groups. Use preallocated virtual disk images inside
the guest instead of adding unpriced Azure disks. Keep the image, filesystem, and
evidence on distinct guest paths. Include the OS disk and temporary evidence
storage in the estimate.

Before admission, calculate:

```text
available compute allowance =
    USD 10 - incurred cost estimate - bounded noncompute costs - cleanup reserve
maximum admitted runtime =
    available compute allowance / current complete hourly compute rate
```

Calculate disk costs separately, including minimum billing increments. Apply
runtime limits across both platforms, guest preparation, and retries. Do not
count account credits as an increase to the ceiling.

Use bounded Azure Run Command operations and a cleanup watchdog outside the
test process. The operator must approve and verify the watchdog before starting
the lab. A test failure must not stop that watchdog. If it cannot be established,
do not provision. Do not promise an exact provider-enforced spending cap:
[Azure budgets](https://learn.microsoft.com/en-us/azure/cost-management-billing/costs/tutorial-acm-create-budgets)
do not stop consumption, and cost reports can arrive late.

Prefer [Azure Run Command](https://learn.microsoft.com/en-us/azure/virtual-machines/windows/run-command-managed)
without public inbound access. Confirm VM-agent connectivity and a private
payload-transfer method before provisioning. Do not silently add NAT, Bastion,
public ingress, storage accounts, or other paid resources.
Run Command commonly starts with privileged guest authority. Use the approved
non-administrator test identity for the application probes. Pass any necessary
credential through protected parameters, never source code or command logs.

Do not run `az vm create` until the exact resource and cost approval exists.
Use explicit `--subscription` and the approved group on all later CLI commands.
Provisioning automation is deliberately not included in this lab.

## Prepare one case

Build the test payload with the repository's pinned tools before a fault.
Use the normal checkout and launcher inside the guest. Keep build artifacts,
SQLite state for media tests, and evidence outside the disposable fault volume.
Do not install tools without the approved guest-setup scope.

The operator must provide:

- A fresh fixed, nonsparse VHDX on Windows, or a preallocated loop image on Linux.
- An NTFS or ext4 filesystem with usable capacity from 32 MiB through 512 MiB.
- An empty, private, normal directory on that volume for generated fixtures.
- An empty, private evidence directory on a separate healthy local volume.
- At least 128 MiB of backing-host free space in addition to the preallocated
  image. This is a minimum guard, not a VM sizing recommendation.
- A second approved image and empty root for the wrong-volume case.

Use a fresh descriptor and fresh fixture roots for each case. The runner creates
`case-started.json` with create-new semantics. A retry must not reuse it.
Keep operator output and test-host logs outside the case evidence directory
until initialization finishes. A log created there before initialization makes
that directory nonempty and must be rejected.

On Windows, use Disk Management to create and attach a fixed VHDX. Verify the
exact image-to-disk association before partitioning or formatting. Assign an
unused drive letter. Never select a disk by a guessed number. Create a private
fixture directory with access for the approved test user. Do not grant
`Everyone` write access. See the
[Windows VHD procedure](https://learn.microsoft.com/en-us/windows-server/storage/disk-management/manage-virtual-hard-disks).

Check image inspection as the normal test account before admitting a case.
The tested Azure Windows Server 2022 image rejected `Get-DiskImage` with
`CimJob_BrokenCimSession` and `Access denied`. The test-only inspector now uses
read-only `OpenVirtualDisk`, `GetVirtualDiskInformation`, and
[`GetStorageDependencyInformation`](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/nf-virtdisk-getstoragedependencyinformation)
queries instead of CIM. It compares the backing image's full native file
identity with the dependency returned for the fixture volume. It rejects
administrator tokens, dynamic or sparse images, remote dependencies, unsafe
volume names, and a different backing image.

Azure reported dependency flags `0x442` for a fixed image on the running OS
volume. The
[`REMOVABLE` flag](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-dependent_disk_flag)
describes the backing disk. The inspector accepts that flag only when the image
host has the same native volume identity as the running OS and reports a fixed
local drive. This does not allow the fixture to use the OS volume. The fixture
must remain on the separate, bounded VHDX.

The approved retry verified actual Rust initialization under the normal account.
It also rejected a different attached VHDX without writing its descriptor.
These are inspection results, not passing volume fault cases. Check both
outcomes before starting faults. Do not elevate the application probe or omit
the image association check to obtain a result.

The later approved Windows run passed publication, receive, normalization,
database-full, and detached-backup cases. Wrong-volume substitution failed
after the original volume returned. A fresh-root retry did not produce a
passing result or an exported archive. See the
[recorded Windows run](crash-recovery.md#approved-windows-volume-run-2026-11-07)
for exact evidence, limits, and verified cleanup. Do not treat successful
inspection or operator acknowledgments as a passing recovery case.

A subsequent [targeted Windows run](crash-recovery.md#targeted-windows-diagnosis-and-recovery-2026-11-07)
identified the operator's partition-type filter defect. The corrected
procedure passed an automated repeat with fresh roots and images. All six
Windows virtual-volume cases now have passing evidence across the approved
runs. This does not complete the broader acceptance boundary.

On Linux, preallocate an image with actual writes, format only that image as
ext4 with `-m 0 -E nodiscard,lazy_itable_init=0,lazy_journal_init=0`, and
associate it with `losetup --find --show --nooverlap`. The
[`nodiscard` option](https://man7.org/linux/man-pages/man8/mke2fs.8.html)
prevents formatting from discarding the allocated blocks. Verify that the
image remains nonsparse after formatting. Record the
returned loop device and its backing-file identity. Mount that exact device at
a fresh lab mount point. Create a private fixture directory for the normal test
user. The user must be able to inspect `findmnt`, `losetup`, and `df` results.
If backing-file inspection is denied, stop rather than skip validation.
See [losetup](https://man7.org/linux/man-pages/man8/losetup.8.html).

Initialization checks the backing image association, native root and image
identities, private root permissions, capacity, filesystem, and nonprivileged
test identity. It rejects the system volume and the image's own host volume.
A copied owner marker does not satisfy these checks.

Example Windows initialization, with paths replaced by the approved lab paths:

```powershell
.\scripts\crash-recovery-lab.ps1 -Action initialize `
  -Consent disposable-volume-faults -Run case-1 `
  -Root V:\fixture -Image C:\DomusLab\case-1.vhdx `
  -Evidence C:\DomusLab\evidence-case-1
```

Use `-Replacement W:\fixture -ReplacementImage C:\DomusLab\case-1-other.vhdx`
when preparing the wrong-volume case. The second fixture root must also be empty.

On Ubuntu, the equivalent wrapper is:

```sh
python3 scripts/crash-recovery-lab.py initialize \
  --consent disposable-volume-faults --run case-1 \
  --root /mnt/domus-case-1/fixture --image /var/tmp/domus-lab/case-1.img \
  --evidence /var/tmp/domus-lab/evidence-case-1
```

Use `--replacement` and `--replacement-image` for the optional second volume.
Initialization writes `lab.json` inside the evidence directory.

## Run filesystem-full cases

Select exactly one of `publication_full`, `receive_full`, `normalization_full`,
or `database_full`:

```powershell
.\scripts\crash-recovery-lab.ps1 -Action run `
  -Consent disposable-volume-faults -Case publication_full `
  -Descriptor C:\DomusLab\evidence-case-1\lab.json
```

```sh
python3 scripts/crash-recovery-lab.py run \
  --consent disposable-volume-faults --case publication_full \
  --descriptor /var/tmp/domus-lab/evidence-case-1/lab.json
```

The parent waits until the application has written a partial output. It writes
real filler bytes on the bounded disposable filesystem, then releases the child
to encounter an actual application storage-full error. It waits for the child's
durable error evidence, terminates that exact process, and reaps it.
Only the exact owned filler is truncated to release space. It is not deleted.
The parent restarts recovery in a fresh process with the same database and roots.
Recovery waits for real lease expiry, without timestamp edits.

The publication and receive cases must reject repair or unsealed input adoption.
Normalization can create a fresh generation-bound run but must preserve all
earlier files. All cases compare interrupted file identities, sizes, and hashes.
SQLite checks use a separate full local volume for database state. They verify
the failed admission's job and event counts after restart, then prove a new
admission can succeed after space is released.

The maximum filler limit is 512 MiB. Sparse files and unsupported filesystems
are rejected. Reaching a bound without the required native error is failure,
not evidence of a full filesystem. Partial files and filler remain after
unexpected failures for operator inspection.

## Run volume-unavailability cases

Use `backup_detached` or `backup_wrong_volume`. The test writes a bounded
operator request into the healthy evidence directory and waits for a
run-bound continuation signal.

For `detach-backup.request.json`, detach or unmount the exact original approved
volume while no application operation owns it. Do not force a busy unmount.
Linux lazy detach is not sufficient proof that storage is unavailable.
For `swap-backup.request.json`, present the separately approved replacement at
the original drive letter or mount path. The fixture subdirectory name must
match. Do not copy the original marker or rebind the application root.

After verifying the requested guest state, signal the waiting parent:

```powershell
.\scripts\crash-recovery-lab.ps1 -Action continue `
  -Consent disposable-volume-faults -Step detach-backup `
  -Descriptor C:\DomusLab\evidence-case-1\lab.json
```

```sh
python3 scripts/crash-recovery-lab.py continue \
  --consent disposable-volume-faults --step detach-backup \
  --descriptor /var/tmp/domus-lab/evidence-case-1/lab.json
```

Use `swap-backup` for the replacement step. The parent verifies the actual
condition before starting a child. An error inspecting a still-present volume
does not pass the detached-volume case.

The child must persist `backup_pending` and preserve primary and staging data.
For `return-original-backup.request.json`, return the exact original volume.
For a swap, also return the replacement to its original mount. Signal with
`return-original-backup`. A fresh process must finish with a new returned claim.
The replacement's sentinel and all unrelated bytes must remain unchanged.

On Windows, verify that both restored drive-letter paths are usable before
acknowledging the return. Successful mount or drive-letter commands alone do
not prove path readiness. Do not select the original partition with
`Where-Object Type -eq Basic`. The tested Windows fixture reported `IFS`, so
that filter selected nothing and left the remounted original at `E:` instead
of `V:`.

Before detachment, record the original partition number from its approved
drive letter. Verify that its disk is the one returned for the exact approved
image. After mounting that same image again, query its current disk and select
the recorded partition with
[`Get-Partition -DiskNumber ... -PartitionNumber ...`](https://learn.microsoft.com/en-us/powershell/module/storage/get-partition).
Reject boot or system disks, ambiguous bindings, and an occupied target letter.
Restore the approved letter on that exact partition. Never guess a disk or
partition number. Keep both fixture-path checks and the parent's native binding
and inventory checks before recovery.

Each operator wait has a bounded deadline. Missed deadlines and refused
detach operations are blocked or failed cases, never passes.

## Evidence and cleanup

Retain `lab.json`, `case-started.json`, `filler.json` where applicable,
`operation-error.json` where applicable, the private child logs, and `result.json`.
Keep generated databases, media, and recovery files private. Export only a
redacted summary to GitHub. Include the exact tested commit and dirty status,
guest OS and filesystem, selected test command, native error code, process
termination, recovery assertions, and cleanup result.

Backup recovery logs identify restored-volume validation, original-backup
inventory, completion-child startup, and completion verification separately.
Root and backing-image checks report the path and expected native identity
before inspection, then the observed identity when available. Diagnostic
errors include the failed operation and path while retaining the original
error source and native code. Inventory errors name the affected directory
or file. Child-start errors distinguish log creation, handle duplication,
executable lookup, and process creation. Keep these paths and native identities
in private logs; publish only a redacted summary.

Do not compress a live SQLite database as a substitute for exporting test
results. A failed operator must still preserve immutable logs and result files
separately. Export database state only after the exact owned processes release
it, using the approved cleanup window. Record an export failure explicitly.
Never extend the VM deadline to recover more evidence.

Do not claim volume evidence from wrapper self-tests or the ordinary ignored
test count. Missing prerequisites are explicit errors when a case is selected.
The ordinary CI workflow runs only wrapper rejection checks and unprivileged
Rust tests.

After exporting evidence, the separately approved operator cleans up only the
recorded fixture roots, mounts, images, and task-owned Azure resources. Check
the exact device-to-image association again before each detach. Never detach
all loop devices or delete a guessed disk.

Delete the approved task-owned Azure group, then verify resource deletion:

```powershell
az group delete --subscription <approved-subscription> --name <approved-lab-group> --yes
az group exists --subscription <approved-subscription> --name <approved-lab-group>
```

Confirm deletion of the recorded disks and network resources. Report failures
immediately. A stopped or deallocated VM can still incur disk and other costs.
Do not start the second platform until cleanup of the first has been verified.

Issue #22 stays open for unperformed physical, managed-production-TLS,
installed-service, reboot, and power-loss acceptance.
