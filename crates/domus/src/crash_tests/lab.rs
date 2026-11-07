// Explicitly selected probes for operator-owned disposable volumes, never normal CI.
use super::*;
use crate::infrastructure::filesystem::{LibraryDirectory, is_unsafe_link, native_root_identity};
use cap_fs_ext::OpenOptionsFollowExt;
use serde_json::json;
use std::io;

const CONSENT: &str = "disposable-volume-faults";
const MAX_VOLUME_BYTES: u64 = 512 * 1024 * 1024;
const HOST_RESERVE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundRoot {
    path: PathBuf,
    identity: String,
}

impl BoundRoot {
    fn bind(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        require_lab_path(path)?;
        let opened = LibraryDirectory::open(path)?;
        let identity = opened.native_root().identity.clone();
        crate::infrastructure::FilePermissions::current()?
            .check(&opened.into_directory().into_std_file(), true)?;
        Ok(Self {
            path: path.to_owned(),
            identity,
        })
    }

    fn check(&self) -> TestResult {
        self.open()?;
        Ok(())
    }

    fn open(&self) -> Result<cap_std::fs::Dir, Box<dyn std::error::Error>> {
        eprintln!(
            "lab root check: path={}, expected={}",
            self.path.display(),
            self.identity
        );
        with_path_context("validate bound root path", &self.path, require_lab_path(&self.path))?;
        let opened = with_path_context(
            "open bound root",
            &self.path,
            LibraryDirectory::open(&self.path),
        )?;
        let observed = &opened.native_root().identity;
        eprintln!(
            "lab root identity: path={}, expected={}, observed={observed}",
            self.path.display(),
            self.identity
        );
        if observed != &self.identity {
            return Err(format!(
                "lab directory identity changed at {}: expected {}, observed {observed}",
                self.path.display(),
                self.identity
            )
            .into());
        }
        let directory = opened.into_directory();
        let handle = with_path_context(
            "clone bound root handle",
            &self.path,
            directory.try_clone(),
        )?;
        with_path_context(
            "check bound root permissions",
            &self.path,
            crate::infrastructure::FilePermissions::current()?
                .check(&handle.into_std_file(), true),
        )?;
        Ok(directory)
    }
}

fn require_lab_path(path: &Path) -> TestResult {
    crate::infrastructure::filesystem::require_local_disk(path)?;
    let mut checkout = std::env::current_dir()?;
    while !checkout.join(".git").try_exists()? {
        if !checkout.pop() {
            return Err("lab probes require their source checkout".into());
        }
    }
    if !path.is_absolute() {
        return Err("lab roots must be absolute and outside the checkout".into());
    }
    let root = LibraryDirectory::open(path)?;
    let checkout = LibraryDirectory::open(&checkout)?;
    if root.native_root().overlaps(checkout.native_root()) {
        return Err("lab roots must not overlap the checkout".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Volume {
    root: BoundRoot,
    image: PathBuf,
    image_identity: String,
    capacity: u64,
}

#[derive(Debug, Deserialize)]
struct ObservedVolume {
    capacity: u64,
    filesystem: String,
    host_available: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lab {
    consent: String,
    run: String,
    evidence: BoundRoot,
    volume: Volume,
    replacement: Option<Volume>,
    max_fill_bytes: u64,
}

#[cfg(unix)]
fn output(
    program: &str,
    arguments: &[&std::ffi::OsStr],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let result = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "{program} lab inspection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(result.stdout)
}

fn image_identity(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("lab image has no parent")?;
    with_path_context("open backing image parent", parent, LibraryDirectory::open(parent))?;
    if is_unsafe_link(&with_path_context(
        "read backing image metadata",
        path,
        std::fs::symlink_metadata(path),
    )?) {
        return Err("lab image must not be a link or reparse point".into());
    }
    let file = with_path_context("open backing image", path, File::open(path))?;
    if !with_path_context("read open backing image metadata", path, file.metadata())?.is_file() {
        return Err("lab image must be a regular file".into());
    }
    with_path_context("query backing image identity", path, native_file_identity(&file))
}

fn volume_id(identity: &str) -> Result<&str, Box<dyn std::error::Error>> {
    let mut parts = identity.split(':');
    let kind = parts.next().ok_or("missing native identity kind")?;
    let volume = parts.next().ok_or("missing native volume identity")?;
    if !matches!(kind, "win128" | "unix") || volume.len() != 16 || parts.next().is_none() {
        return Err("invalid native volume identity".into());
    }
    Ok(volume)
}

impl Volume {
    fn bind(root: &Path, image: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let bound = BoundRoot::bind(root)?;
        if std::fs::read_dir(root)?.next().transpose()?.is_some() {
            return Err("initial lab fixture root must be empty".into());
        }
        let identity = image_identity(image)?;
        let observed = observe_volume(root, image)?;
        let volume = Self {
            root: bound,
            image: image.to_owned(),
            image_identity: identity,
            capacity: observed.capacity,
        };
        volume.check()?;
        Ok(volume)
    }

    fn check_image(&self) -> TestResult {
        eprintln!(
            "lab image check: path={}, expected={}",
            self.image.display(),
            self.image_identity
        );
        let observed = image_identity(&self.image)?;
        eprintln!(
            "lab image identity: path={}, expected={}, observed={observed}",
            self.image.display(),
            self.image_identity
        );
        if observed != self.image_identity {
            return Err(format!(
                "lab backing image identity changed at {}: expected {}, observed {observed}",
                self.image.display(),
                self.image_identity
            )
            .into());
        }
        Ok(())
    }

    fn check(&self) -> TestResult {
        self.root.check()?;
        self.check_image()?;
        let observed = with_path_context(
            format!("inspect volume backed by {}", self.image.display()),
            &self.root.path,
            observe_volume(&self.root.path, &self.image),
        )?;
        with_path_context(
            "validate volume observation",
            &self.root.path,
            validate_observation(&observed, self.capacity),
        )?;
        #[cfg(windows)]
        let system = PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is missing")?);
        #[cfg(unix)]
        let system = PathBuf::from("/");
        let system_identity = with_path_context(
            "query system volume identity",
            &system,
            native_root_identity(&system),
        )?;
        if volume_id(&self.root.identity)? == volume_id(&system_identity)?
            || volume_id(&self.root.identity)? == volume_id(&self.image_identity)?
        {
            return Err("lab target is a system or backing-image volume".into());
        }
        Ok(())
    }

    fn check_detached(&self) -> TestResult {
        self.check_image()?;
        #[cfg(windows)]
        if inspect_windows_volume(&self.root.path, &self.image, true)?["detached"] != true {
            return Err("native inspection did not confirm image detachment".into());
        }
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            let loops = serde_json::from_slice(&output(
                "losetup",
                &[
                    OsStr::new("--json"),
                    OsStr::new("--list"),
                    OsStr::new("--associated"),
                    self.image.as_os_str(),
                    OsStr::new("--output"),
                    OsStr::new("NAME,MAJ:MIN"),
                ],
            )?)?;
            let mounts = serde_json::from_slice(&output(
                "findmnt",
                &[
                    OsStr::new("--json"),
                    OsStr::new("--list"),
                    OsStr::new("--kernel"),
                    OsStr::new("--output"),
                    OsStr::new("MAJ:MIN"),
                ],
            )?)?;
            check_unmounted_loop_devices(&loops, &mounts)?;
        }
        self.check_image()
    }
}

fn check_unmounted_loop_devices(loops: &serde_json::Value, mounts: &serde_json::Value) -> TestResult {
    let devices = loops["loopdevices"]
        .as_array()
        .ok_or("missing associated loop devices")?;
    let mounted = mounts["filesystems"]
        .as_array()
        .ok_or("missing kernel mount information")?
        .iter()
        .map(|entry| {
            entry["maj:min"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("missing mounted device identity")
        })
        .collect::<Result<Vec<_>, _>>()?;
    for entry in devices {
        let device = entry["maj:min"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("missing associated loop device identity")?;
        if mounted.contains(&device) {
            return Err("approved image is still mounted; directory absence is not detachment".into());
        }
    }
    Ok(())
}

fn validate_observation(observed: &ObservedVolume, capacity: u64) -> TestResult {
    if !(32 * 1024 * 1024..=MAX_VOLUME_BYTES).contains(&capacity)
        || observed.capacity != capacity
        || !matches!(observed.filesystem.as_str(), "NTFS" | "ext4")
        || observed.host_available < HOST_RESERVE_BYTES
    {
        return Err("lab volume size, filesystem, or backing-host reserve is unsafe".into());
    }
    Ok(())
}

#[cfg(windows)]
fn observe_volume(root: &Path, image: &Path) -> Result<ObservedVolume, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(inspect_windows_volume(root, image, false)?)?)
}

#[cfg(windows)]
fn inspect_windows_volume(
    root: &Path,
    image: &Path,
    detached: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    // Arguments are passed as environment values, not interpolated into PowerShell code.
    let script = include_str!("windows_volume_inspection.ps1");
    let result = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("DOMUS_INSPECT_ROOT", root)
        .env("DOMUS_INSPECT_IMAGE", image)
        .env("DOMUS_INSPECT_DETACHED", if detached { "1" } else { "0" })
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "volume inspection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}

#[cfg(unix)]
fn observe_volume(root: &Path, image: &Path) -> Result<ObservedVolume, Box<dyn std::error::Error>> {
    use std::ffi::OsStr;
    use std::os::unix::fs::MetadataExt;
    if output("id", &[OsStr::new("-u")])? == b"0\n" {
        return Err("run application tests without root privileges".into());
    }
    let mount: serde_json::Value = serde_json::from_slice(&output(
        "findmnt",
        &[
            OsStr::new("--json"),
            OsStr::new("--target"),
            root.as_os_str(),
            OsStr::new("--output"),
            OsStr::new("SOURCE,FSTYPE"),
        ],
    )?)?;
    let mounts = mount["filesystems"]
        .as_array()
        .ok_or("missing mount information")?;
    if mounts.len() != 1 {
        return Err("lab target does not have one exact mount".into());
    }
    let device = mounts[0]["source"].as_str().ok_or("missing mount source")?;
    let loops: serde_json::Value = serde_json::from_slice(&output(
        "losetup",
        &[
            OsStr::new("--json"),
            OsStr::new("--list"),
            OsStr::new("--output"),
            OsStr::new("NAME,BACK-FILE"),
        ],
    )?)?;
    let devices = loops["loopdevices"]
        .as_array()
        .ok_or("missing loop device information")?;
    let matched = devices
        .iter()
        .filter(|item| item["name"].as_str() == Some(device))
        .collect::<Vec<_>>();
    if matched.len() != 1 {
        return Err("lab filesystem must be on one operator-created loop device".into());
    }
    let backing = matched[0]["back-file"]
        .as_str()
        .ok_or("loop backing path unavailable; operator inspection permission is required")?;
    if image_identity(Path::new(backing))? != image_identity(image)? {
        return Err("loop backing file does not match the approved image".into());
    }
    let metadata = File::open(image)?.metadata()?;
    if metadata.blocks().saturating_mul(512) < metadata.len() {
        return Err("use a preallocated, nonsparse loop image".into());
    }
    let space = |path: &Path| -> Result<(u64, u64), Box<dyn std::error::Error>> {
        let bytes = output(
            "df",
            &[
                OsStr::new("-B1"),
                OsStr::new("--output=size,avail"),
                path.as_os_str(),
            ],
        )?;
        let text = std::str::from_utf8(&bytes)?;
        let values = text
            .lines()
            .nth(1)
            .ok_or("missing df data")?
            .split_whitespace()
            .collect::<Vec<_>>();
        if values.len() != 2 {
            return Err("unexpected df data".into());
        }
        Ok((values[0].parse()?, values[1].parse()?))
    };
    Ok(ObservedVolume {
        capacity: space(root)?.0,
        filesystem: mounts[0]["fstype"]
            .as_str()
            .ok_or("missing filesystem type")?
            .to_owned(),
        host_available: space(image)?.1,
    })
}

impl Lab {
    fn load() -> Result<Self, Box<dyn std::error::Error>> {
        require_consent()?;
        let descriptor =
            std::env::var_os("DOMUS_LAB_DESCRIPTOR").ok_or("DOMUS_LAB_DESCRIPTOR is required")?;
        let file = File::open(descriptor)?;
        if file.metadata()?.len() > 16_384 {
            return Err("lab descriptor exceeds its limit".into());
        }
        let lab: Self = serde_json::from_reader(file)?;
        lab.validate()?;
        Ok(lab)
    }

    fn validate(&self) -> TestResult {
        if self.consent != CONSENT
            || self.run.is_empty()
            || self.run.len() > 64
            || !self
                .run
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || self.max_fill_bytes == 0
            || self.max_fill_bytes > MAX_VOLUME_BYTES
            || self.max_fill_bytes < self.volume.capacity
        {
            return Err("invalid lab consent, run identity, or filler limit".into());
        }
        with_path_context(
            "validate evidence root",
            &self.evidence.path,
            self.evidence.check(),
        )?;
        with_path_context(
            "validate original volume",
            &self.volume.root.path,
            self.volume.check(),
        )?;
        if volume_id(&self.evidence.identity)? == volume_id(&self.volume.root.identity)? {
            return Err("lab evidence must be on a healthy separate local volume".into());
        }
        if let Some(replacement) = &self.replacement {
            with_path_context(
                "validate replacement volume",
                &replacement.root.path,
                replacement.check(),
            )?;
            if volume_id(&replacement.root.identity)? == volume_id(&self.volume.root.identity)?
                || volume_id(&replacement.root.identity)? == volume_id(&self.evidence.identity)?
            {
                return Err("replacement requires a separate disposable volume".into());
            }
        }
        Ok(())
    }

    fn record(&self, name: &str, value: &impl Serialize) -> TestResult {
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(cap_fs_ext::FollowSymlinks::No);
        let file = self.evidence.open()?.open_with(name, &options)?.into_std();
        serde_json::to_writer_pretty(&file, value)?;
        file.sync_all()?;
        Ok(())
    }

    fn start(&self, case: &str) -> TestResult {
        let revision = Command::new("git")
            .args(["--no-pager", "rev-parse", "HEAD"])
            .output()?;
        let status = Command::new("git")
            .args(["--no-pager", "status", "--porcelain"])
            .output()?;
        if !revision.status.success() || !status.status.success() {
            return Err("cannot record the exact tested checkout revision".into());
        }
        self.record(
            "case-started.json",
            &json!({
                "case":case,"revision":String::from_utf8(revision.stdout)?.trim(),
                "dirty":!status.stdout.is_empty(),"os":std::env::consts::OS,
                "filesystem":observe_volume(&self.volume.root.path,&self.volume.image)?.filesystem
            }),
        )
    }

    fn operator_step(&self, name: &str) -> TestResult {
        self.record(
            &format!("{name}.request.json"),
            &json!({"run":self.run,"step":name}),
        )?;
        let signal = format!("{name}.continue");
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
            match self.evidence.open()?.open_with(&signal, &options) {
                Ok(mut file) => {
                    if file.metadata()?.len() > 128 {
                        return Err("unsafe operator continuation signal".into());
                    }
                    let mut text = String::new();
                    file.read_to_string(&mut text)?;
                    if text.trim() != self.run {
                        return Err("operator signal has a different run identity".into());
                    }
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "operator step {name} exceeded its deadline; evidence retained"
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn require_consent() -> TestResult {
    check_consent(std::env::var("DOMUS_LAB_CONSENT").ok().as_deref())
}

fn check_consent(value: Option<&str>) -> TestResult {
    if value != Some(CONSENT) {
        return Err("explicit DOMUS_LAB_CONSENT=disposable-volume-faults is required".into());
    }
    Ok(())
}

#[test]
#[ignore = "operator-only initialization on empty, inspected disposable volumes"]
fn volume_lab_initialize() -> TestResult {
    require_consent()?;
    let env_path = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} is required"))
    };
    let evidence = BoundRoot::bind(&env_path("DOMUS_LAB_EVIDENCE")?)?;
    if std::fs::read_dir(&evidence.path)?
        .next()
        .transpose()?
        .is_some()
    {
        return Err("lab evidence directory must initially be empty".into());
    }
    let replacement = match (
        std::env::var_os("DOMUS_LAB_REPLACEMENT"),
        std::env::var_os("DOMUS_LAB_REPLACEMENT_IMAGE"),
    ) {
        (None, None) => None,
        (Some(root), Some(image)) => Some(Volume::bind(Path::new(&root), Path::new(&image))?),
        _ => return Err("both replacement root and image are required".into()),
    };
    let lab = Lab {
        consent: CONSENT.into(),
        run: std::env::var("DOMUS_LAB_RUN")?,
        evidence,
        volume: Volume::bind(&env_path("DOMUS_LAB_ROOT")?, &env_path("DOMUS_LAB_IMAGE")?)?,
        replacement,
        max_fill_bytes: MAX_VOLUME_BYTES,
    };
    lab.validate()?;
    lab.record("lab.json", &lab)
}

fn retained_directory(parent: &Path) -> Result<TempDir, Box<dyn std::error::Error>> {
    let mut directory = tempfile::tempdir_in(parent)?;
    directory.disable_cleanup(true);
    Ok(directory)
}

fn prepare_volume(
    lab: &Lab,
    action: Action,
    point: &'static str,
) -> Result<Case, Box<dyn std::error::Error>> {
    let state = retained_directory(&lab.evidence.path)?;
    let library = retained_directory(if matches!(action, Action::Publish) {
        &lab.volume.root.path
    } else {
        &lab.evidence.path
    })?;
    let scratch = retained_directory(if matches!(action, Action::Receive | Action::Normalize) {
        &lab.volume.root.path
    } else {
        &lab.evidence.path
    })?;
    let mut fixture = Fixture::build_with_inputs(
        AudioFormat::Mp3,
        true,
        true,
        !matches!(action, Action::Receive),
        None,
        None,
        FixtureInputs {
            directories: Some([state, library, scratch]),
            source: Some(super::super::profiles::frames(
                &[320; 128],
                false,
                false,
                0,
                false,
            )),
            ..Default::default()
        },
    )?;
    let run = if matches!(action, Action::Publish) {
        Some(normalize_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            &fixture.cover,
        )?)
    } else {
        None
    };
    case_from_fixture(fixture, action, point, run)
}

struct Filler {
    directory: cap_std::fs::Dir,
    identity: String,
}

impl Filler {
    fn fill(lab: &Lab) -> Result<Self, Box<dyn std::error::Error>> {
        lab.validate()?;
        let directory = lab.volume.root.open()?;
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(cap_fs_ext::FollowSymlinks::No);
        let mut file = directory
            .open_with("allocated-filler", &options)?
            .into_std();
        let identity = native_file_identity(&file)?;
        let filler = Self {
            directory,
            identity,
        };
        let (written, native_code) = Self::allocate_until_full(&mut file, lab.max_fill_bytes)?;
        match file.sync_all() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::StorageFull => {}
            Err(error) => return Err(error.into()),
        }
        lab.record(
            "filler.json",
            &json!({"written":written,"identity":filler.identity,"native_storage_full":true,"native_code":native_code}),
        )?;
        Ok(filler)
    }

    fn allocate_until_full(
        file: &mut impl Write,
        limit: u64,
    ) -> Result<(u64, i32), Box<dyn std::error::Error>> {
        if limit == 0 || limit > MAX_VOLUME_BYTES {
            return Err("unsafe filler byte limit".into());
        }
        let chunk = [0xa5; 65_536];
        let mut written = 0u64;
        loop {
            if written >= limit {
                return Err(
                    "filler limit reached without actual filesystem exhaustion; filler retained"
                        .into(),
                );
            }
            match file.write(&chunk[..(limit - written).min(chunk.len() as u64) as usize]) {
                Ok(0) => return Err("filler made no write progress; filler retained".into()),
                Ok(count) => written += count as u64,
                Err(error) if error.kind() == io::ErrorKind::StorageFull => {
                    return Ok((
                        written,
                        error
                            .raw_os_error()
                            .ok_or("storage-full error has no native code")?,
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn release(self, lab: &Lab) -> TestResult {
        lab.volume.check()?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).follow(cap_fs_ext::FollowSymlinks::No);
        let file = self
            .directory
            .open_with("allocated-filler", &options)?
            .into_std();
        if native_file_identity(&file)? != self.identity {
            return Err("owned filler changed; refusing truncation".into());
        }
        file.set_len(0)?;
        file.sync_all()?;
        Ok(())
    }
}

pub(super) fn fault_child(store: &mut Store, descriptor: &Descriptor) -> TestResult {
    let lab = Lab::load()?;
    let channel = TcpStream::connect(std::env::var("DOMUS_CRASH_CHANNEL")?)?;
    channel.set_read_timeout(Some(Duration::from_secs(300)))?;
    channel.set_write_timeout(Some(START_LIMIT))?;
    super::super::PROBE.set(Some(super::super::Probe {
        target: descriptor.point.clone(),
        channel,
        fail_write: false,
        resume_once: true,
    }));
    let result = perform(store, descriptor);
    let Err(error) = result else {
        return Err("full-volume operation unexpectedly succeeded".into());
    };
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
    let mut native = None;
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<io::Error>()
            && error.kind() == io::ErrorKind::StorageFull
        {
            native = error.raw_os_error();
            break;
        }
        current = error.source();
    }
    let native =
        native.ok_or_else(|| format!("operation did not report native storage-full: {error}"))?;
    lab.record("operation-error.json", &json!({"kind":"StorageFull","native_code":native,"action":format!("{:?}",descriptor.action)}))?;
    wait_for_fault_termination()
}

fn wait_for_fault_termination() -> TestResult {
    super::super::PROBE.with_borrow_mut(|probe| -> TestResult {
        let probe = probe.as_mut().ok_or("lab probe is missing")?;
        probe.target = "volume_failed".into();
        probe.resume_once = false;
        Ok(())
    })?;
    super::super::checkpoint("volume_failed")?;
    Err("failed operation must be terminated by its parent".into())
}

fn full_volume(action: Action, point: &'static str) -> TestResult {
    let lab = Lab::load()?;
    lab.start(&format!("{action:?}:{point}"))?;
    let case = prepare_volume(&lab, action, point)?;
    lab.record(
        "fixture.json",
        &json!({"descriptor":case.descriptor,"checkpoint":point}),
    )?;
    let before_source = inventory(case.fixture.scratch.path())?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(
        &case.descriptor,
        "volume_fault",
        Some(listener.local_addr()?),
    )?;
    let mut channel = child.ready(&listener, point)?;
    let filler = Filler::fill(&lab)?;
    channel.write_all(b"C")?;
    let mut report = String::new();
    channel.set_read_timeout(Some(START_LIMIT))?;
    BufReader::new(&channel).take(128).read_line(&mut report)?;
    assert_eq!(report.trim(), "volume_failed");
    child.terminate()?;
    drop(channel);
    filler.release(&lab)?;
    let interrupted = inventory(&lab.volume.root.path)?;
    lab.record("interrupted.json", &interrupted)?;
    let mut retry = OwnedChild::start(&case.descriptor, "recover", None)?;
    retry.finish()?;
    assert_preserved(&interrupted, &inventory(&lab.volume.root.path)?, false);
    lab.record("recovered.json", &inventory(&lab.volume.root.path)?)?;
    if matches!(action, Action::Publish) {
        assert_eq!(inventory(case.fixture.scratch.path())?, before_source);
    }
    integrity(&case.fixture.database)?;
    lab.record("result.json", &json!({"case":format!("{action:?}"),"status":"passed","process_terminated":true,"recovered_in_new_process":true}))
}

#[test]
#[ignore = "requires an approved disposable volume lab; never run through --ignored discovery"]
fn volume_lab_publication_full() -> TestResult {
    full_volume(Action::Publish, "publication_writing")
}

#[test]
#[ignore = "requires an approved disposable volume lab; never run through --ignored discovery"]
fn volume_lab_receive_full() -> TestResult {
    full_volume(Action::Receive, "receive_writing")
}

#[test]
#[ignore = "requires an approved disposable volume lab; never run through --ignored discovery"]
fn volume_lab_normalization_full() -> TestResult {
    full_volume(Action::Normalize, "normalization_writing")
}

pub(super) fn database_child(store: &mut Store, descriptor: &Descriptor) -> TestResult {
    let lab = Lab::load()?;
    let channel = TcpStream::connect(std::env::var("DOMUS_CRASH_CHANNEL")?)?;
    channel.set_read_timeout(Some(Duration::from_secs(300)))?;
    channel.set_write_timeout(Some(START_LIMIT))?;
    super::super::PROBE.set(Some(super::super::Probe {
        target: "database_ready".into(),
        channel,
        fail_write: false,
        resume_once: true,
    }));
    super::super::checkpoint("database_ready")?;
    let raw = Connection::open(&descriptor.database)?;
    for _ in 0..4096 {
        let jobs: i64 = raw.query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))?;
        let events = store.last_event_id()?;
        match store.enqueue(1) {
            Ok(_) => {}
            Err(crate::Error::Database(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DiskFull =>
            {
                lab.record(
                    "operation-error.json",
                    &json!({
                        "kind":"SQLiteDiskFull","native_code":error.extended_code,
                        "jobs_before":jobs,"events_before":events
                    }),
                )?;
                return wait_for_fault_termination();
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err("database did not reach actual filesystem exhaustion within the admission limit".into())
}

pub(super) fn database_recover(store: &mut Store, descriptor: &Descriptor) -> TestResult {
    let lab = Lab::load()?;
    let before: serde_json::Value =
        serde_json::from_reader(File::open(lab.evidence.path.join("operation-error.json"))?)?;
    let raw = Connection::open(&descriptor.database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))?,
        before["jobs_before"].as_i64().ok_or("job evidence")?
    );
    assert_eq!(
        store.last_event_id()?,
        before["events_before"].as_i64().ok_or("event evidence")?
    );
    integrity(&descriptor.database)?;
    store.enqueue(1)?;
    integrity(&descriptor.database)
}

#[test]
#[ignore = "requires an approved disposable local database volume"]
fn volume_lab_database_full() -> TestResult {
    let lab = Lab::load()?;
    lab.start("database_full")?;
    let fixture = Fixture::build_with_inputs(
        AudioFormat::Flac,
        true,
        true,
        true,
        None,
        None,
        FixtureInputs {
            directories: Some([
                retained_directory(&lab.volume.root.path)?,
                retained_directory(&lab.evidence.path)?,
                retained_directory(&lab.evidence.path)?,
            ]),
            ..Default::default()
        },
    )?;
    let case = case_from_fixture(fixture, Action::Normalize, "database_ready", None)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(
        &case.descriptor,
        "volume_database",
        Some(listener.local_addr()?),
    )?;
    let mut channel = child.ready(&listener, "database_ready")?;
    let filler = Filler::fill(&lab)?;
    channel.write_all(b"C")?;
    let mut report = String::new();
    BufReader::new(&channel).take(128).read_line(&mut report)?;
    assert_eq!(report.trim(), "volume_failed");
    child.terminate()?;
    drop(channel);
    filler.release(&lab)?;
    let mut retry = OwnedChild::start(&case.descriptor, "volume_database_recover", None)?;
    retry.finish()?;
    lab.record("result.json", &json!({"case":"database_full","status":"passed","process_terminated":true,"recovered_in_new_process":true}))
}

#[test]
#[ignore = "operator must detach and reattach the exact approved backup volume"]
fn volume_lab_backup_detached() -> TestResult {
    backup_volume(false)
}

#[test]
#[ignore = "operator must swap two approved disposable backup volumes"]
fn volume_lab_backup_wrong_volume() -> TestResult {
    backup_volume(true)
}

fn backup_volume(swap: bool) -> TestResult {
    let lab = Lab::load()?;
    lab.start(if swap {
        "wrong_volume"
    } else {
        "detached_backup"
    })?;
    if swap && lab.replacement.is_none() {
        return Err("wrong-volume case requires a separately bound replacement".into());
    }
    let backup = lab.volume.root.path.join("backup");
    std::fs::create_dir(&backup)?;
    if let Some(replacement) = &lab.replacement {
        std::fs::create_dir(replacement.root.path.join("backup"))?;
        std::fs::write(
            replacement.root.path.join("sentinel"),
            b"unrelated generated replacement-volume content",
        )?;
    }
    let fixture = Fixture::build_with_inputs(
        AudioFormat::Flac,
        true,
        true,
        true,
        Some(&backup),
        None,
        FixtureInputs {
            directories: Some([
                retained_directory(&lab.evidence.path)?,
                retained_directory(&lab.evidence.path)?,
                retained_directory(&lab.evidence.path)?,
            ]),
            ..Default::default()
        },
    )?;
    let mut fixture = fixture;
    let run = normalize_album(
        &mut fixture.store,
        &fixture.claim,
        fixture.staging.as_ref().ok_or("staging")?,
        &fixture.cover,
    )?;
    let case = case_from_fixture(fixture, Action::Publish, "unused", Some(run))?;
    let before_staging = inventory(case.fixture.scratch.path())?;
    let before_backup = inventory(&lab.volume.root.path)?;
    let replacement_before = lab
        .replacement
        .as_ref()
        .map(|volume| inventory(&volume.root.path))
        .transpose()?;
    // No application operation owns the backup root during the operator's mount step.
    lab.operator_step(if swap { "swap-backup" } else { "detach-backup" })?;
    if swap {
        let replacement = lab.replacement.as_ref().ok_or("replacement")?;
        if native_root_identity(&lab.volume.root.path)? != replacement.root.identity
            || image_identity(&replacement.image)? != replacement.image_identity
        {
            return Err(
                "replacement at the target path is not the separately approved volume".into(),
            );
        }
        validate_observation(
            &observe_volume(&lab.volume.root.path, &replacement.image)?,
            replacement.capacity,
        )?;
    } else {
        lab.volume.check_detached()?;
        lab.record(
            "detachment.json",
            &json!({"detached":true,"image_identity":lab.volume.image_identity}),
        )?;
        // A live original handle identity or an ordinary existing directory is not drive loss.
        if std::fs::symlink_metadata(&lab.volume.root.path).is_ok() {
            return Err(
                "backup root remains present; volume detachment was not demonstrated".into(),
            );
        }
        match std::fs::symlink_metadata(&lab.volume.root.path) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err("backup root reappeared during detach inspection".into()),
        }
    }
    let mut pending = OwnedChild::start(&case.descriptor, "backup_pending", None)?;
    pending.finish()?;
    assert_eq!(
        case.fixture.store.job_state(case.fixture.job)?.as_deref(),
        Some("backup_pending")
    );
    assert_eq!(inventory(case.fixture.scratch.path())?, before_staging);
    let primary = inventory(case.fixture._library.path())?;
    if let Some(before) = &replacement_before
        && swap
    {
        assert_eq!(inventory(&lab.volume.root.path)?, *before);
    }
    eprintln!("backup recovery: waiting for original-volume return");
    lab.operator_step("return-original-backup")?;
    eprintln!("backup recovery: validating restored lab volumes");
    lab.validate()?;
    eprintln!("backup recovery: checking restored original backup inventory");
    assert_eq!(
        with_path_context(
            "inventory restored original backup",
            &lab.volume.root.path,
            inventory(&lab.volume.root.path),
        )?,
        before_backup,
        "original backup inventory changed after volume return"
    );
    eprintln!("backup recovery: starting completion child");
    let mut complete = OwnedChild::start(&case.descriptor, "backup_complete", None)?;
    let completion = complete.finish();
    with_path_context(
        "wait for backup completion child",
        &complete.log,
        completion,
    )?;
    eprintln!("backup recovery: completion child passed; checking saved state and content");
    assert_eq!(
        case.fixture.store.job_state(case.fixture.job)?.as_deref(),
        Some("complete")
    );
    assert_preserved(&primary, &inventory(case.fixture._library.path())?, false);
    if let (Some(volume), Some(before)) = (&lab.replacement, replacement_before) {
        assert_eq!(inventory(&volume.root.path)?, before);
    }
    integrity(&case.fixture.database)?;
    lab.record("result.json", &json!({"case":if swap {"wrong_volume"} else {"detached_backup"},"status":"passed","fresh_process_continuation":true,"same_original_root":true}))
}

#[test]
fn volume_lab_rejects_unsafe_limits_and_identity_shapes() -> TestResult {
    assert!(check_consent(None).is_err());
    assert!(check_consent(Some("wrong")).is_err());
    check_consent(Some(CONSENT))?;
    assert!(volume_id("marker").is_err());
    for (capacity, filesystem, reserve) in [
        (MAX_VOLUME_BYTES + 1, "NTFS", HOST_RESERVE_BYTES),
        (64 * 1024 * 1024, "tmpfs", HOST_RESERVE_BYTES),
        (64 * 1024 * 1024, "ext4", HOST_RESERVE_BYTES - 1),
    ] {
        assert!(
            validate_observation(
                &ObservedVolume {
                    capacity,
                    filesystem: filesystem.into(),
                    host_available: reserve
                },
                capacity
            )
            .is_err()
        );
    }
    let root = tempdir()?;
    let binding = BoundRoot::bind(root.path())?;
    let mut changed = binding.clone();
    changed.identity.push('0');
    let error = changed
        .check()
        .err()
        .ok_or("changed identity must be rejected")?;
    let message = error.to_string();
    assert!(message.contains(&root.path().display().to_string()), "{message}");
    assert!(message.contains(&format!("expected {}", changed.identity)), "{message}");
    assert!(message.contains(&format!("observed {}", binding.identity)), "{message}");
    binding.check()?;
    Ok(())
}

#[test]
fn volume_lab_diagnostics_preserve_operation_path_and_native_error() -> TestResult {
    let directory = tempdir()?;
    let missing = directory.path().join("missing").join("fixture");
    let native = std::fs::metadata(&missing)
        .err()
        .ok_or("fixture must not exist")?;
    let code = native.raw_os_error();
    assert!(code.is_some());
    let result = with_path_context("open restored root", &missing, Err::<(), _>(native));
    let error = with_path_context("validate original volume", directory.path(), result)
        .err()
        .ok_or("diagnostics must not turn failure into success")?;
    let message = error.to_string();
    assert!(message.contains("validate original volume"), "{message}");
    assert!(message.contains("open restored root"), "{message}");
    assert!(message.contains(&missing.display().to_string()), "{message}");
    let source = error
        .source()
        .and_then(std::error::Error::source)
        .and_then(|source| source.downcast_ref::<io::Error>())
        .ok_or("native I/O error source was lost")?;
    assert_eq!(source.raw_os_error(), code);
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    Ok(())
}

#[test]
fn volume_lab_inventory_reports_the_failed_directory_path() -> TestResult {
    let directory = tempdir()?;
    let missing = directory.path().join("missing");
    let error = inventory(&missing)
        .err()
        .ok_or("missing inventory root must fail")?;
    let context = error
        .downcast_ref::<ProbeError>()
        .ok_or("missing inventory diagnostic")?;
    assert_eq!(context.operation, "read inventory directory");
    assert_eq!(context.path, missing);
    assert_eq!(
        context
            .source
            .downcast_ref::<io::Error>()
            .ok_or("missing native inventory error")?
            .kind(),
        io::ErrorKind::NotFound
    );
    Ok(())
}

#[test]
fn volume_lab_child_start_reports_the_failed_log_path() -> TestResult {
    let directory = tempdir()?;
    let descriptor = directory.path().join("missing").join("crash.json");
    let error = match OwnedChild::start(&descriptor, "backup_complete", None) {
        Err(error) => error,
        Ok(_) => return Err("child must not start without its log directory".into()),
    };
    let context = error
        .downcast_ref::<ProbeError>()
        .ok_or("missing child-start diagnostic")?;
    assert_eq!(context.operation, "create backup_complete child log");
    assert_eq!(context.path, descriptor.with_extension("backup_complete.log"));
    assert_eq!(
        context
            .source
            .downcast_ref::<io::Error>()
            .ok_or("missing native child-start error")?
            .kind(),
        io::ErrorKind::NotFound
    );
    Ok(())
}

#[test]
fn volume_lab_rejects_replaced_roots_and_checkout_paths() -> TestResult {
    assert!(BoundRoot::bind(Path::new("relative")).is_err());
    assert!(BoundRoot::bind(&std::env::current_dir()?).is_err());
    let parent = tempdir()?;
    let path = parent.path().join("fixture");
    std::fs::create_dir(&path)?;
    private::directory(&path)?;
    let root = BoundRoot::bind(&path)?;
    std::fs::rename(&path, parent.path().join("original"))?;
    std::fs::create_dir(&path)?;
    private::directory(&path)?;
    assert!(root.check().is_err());
    Ok(())
}

#[test]
fn volume_lab_detachment_rejects_directory_rename_on_a_mounted_image() -> TestResult {
    let parent = tempdir()?;
    let root = parent.path().join("fixture");
    std::fs::create_dir(&root)?;
    std::fs::rename(&root, parent.path().join("renamed"))?;
    assert!(!root.try_exists()?);
    let loops = json!({"loopdevices":[{"name":"/dev/loop7","maj:min":"7:7"}]});
    let mounted = json!({"filesystems":[{"maj:min":"8:1"},{"maj:min":"7:7"}]});
    assert!(check_unmounted_loop_devices(&loops, &mounted).is_err());
    check_unmounted_loop_devices(&loops, &json!({"filesystems":[{"maj:min":"8:1"}]}))?;
    check_unmounted_loop_devices(&json!({"loopdevices":[]}), &mounted)?;
    for invalid in [json!({}), json!({"filesystems":[{}]})] {
        assert!(check_unmounted_loop_devices(&loops, &invalid).is_err());
    }
    assert!(
        check_unmounted_loop_devices(&json!({"loopdevices":[{}]}), &mounted).is_err()
    );
    Ok(())
}

#[test]
#[cfg(windows)]
fn volume_lab_windows_inspection_rejects_regular_files_without_mutating_them() -> TestResult {
    let directory = tempdir()?;
    let image = directory.path().join("not-a-virtual-disk.vhdx");
    let bytes = b"generated non-VHDX guard fixture";
    std::fs::write(&image, bytes)?;
    let identity = image_identity(&image)?;
    for detached in [false, true] {
        let error = match inspect_windows_volume(directory.path(), &image, detached) {
            Err(error) => error.to_string(),
            Ok(_) => return Err("a regular file must not pass VHDX inspection".into()),
        };
        assert!(
            error.contains("OpenVirtualDisk failed")
                || error.contains("Run application tests without elevation"),
            "{error}"
        );
    }
    assert_eq!(std::fs::read(&image)?, bytes);
    assert_eq!(image_identity(&image)?, identity);
    Ok(())
}

#[test]
fn volume_lab_filler_enforces_the_exact_byte_limit_and_requires_native_failure() -> TestResult {
    struct Writer {
        capacity: u64,
        written: u64,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.written == self.capacity {
                #[cfg(windows)]
                let code = 112;
                #[cfg(unix)]
                let code = 28;
                return Err(io::Error::from_raw_os_error(code));
            }
            let count = (self.capacity - self.written).min(bytes.len() as u64) as usize;
            self.written += count as u64;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut full = Writer {
        capacity: 65_543,
        written: 0,
    };
    assert_eq!(Filler::allocate_until_full(&mut full, 131_072)?.0, 65_543);
    let mut bounded = Writer {
        capacity: 200_000,
        written: 0,
    };
    assert!(Filler::allocate_until_full(&mut bounded, 131_073).is_err());
    assert_eq!(bounded.written, 131_073);
    assert!(Filler::allocate_until_full(&mut bounded, MAX_VOLUME_BYTES + 1).is_err());
    assert_eq!(bounded.written, 131_073);
    assert!(Filler::allocate_until_full(&mut bounded, 0).is_err());
    Ok(())
}
