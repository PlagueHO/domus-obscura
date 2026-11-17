//! Isolated Samba proof bridge to the existing Rust relationship rule.
//! Native paths stay inside the server. The caller retains every supplied
//! descriptor throughout collection; this bridge never opens a media path.

#[path = "../crates/domus/src/infrastructure/filesystem/topology_prototype.rs"]
mod topology;

use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt;

const ROOTS: usize = 4;

#[repr(C)]
pub struct Observation {
    descriptor: i32,
    device: u64,
    inode: u64,
    mount_id: u64,
}

fn bounded_text(path: &str, limit: usize) -> Result<String, Box<dyn Error>> {
    let mut value = String::new();
    File::open(path)?
        .take((limit + 1) as u64)
        .read_to_string(&mut value)?;
    if value.len() > limit {
        return Err(topology::EvidenceError::LimitExceeded.into());
    }
    Ok(value)
}

fn collect(observations: &[Observation], mask: u32) -> Result<[u8; 16], Box<dyn Error>> {
    let before = bounded_text("/proc/self/mountinfo", 1_048_576)?;
    let mounts = topology::parse_mounts(&before)?;
    let mut roots: [Option<topology::Root>; ROOTS] = std::array::from_fn(|_| None);
    for (index, observation) in observations.iter().enumerate() {
        if mask & (1 << index) == 0 {
            continue;
        }
        if observation.descriptor < 0 {
            return Err("missing retained native descriptor".into());
        }
        // The FFI caller holds the descriptor. Only this owned duplicate closes.
        let borrowed = unsafe { BorrowedFd::borrow_raw(observation.descriptor) };
        let file = File::from(borrowed.try_clone_to_owned()?);
        let metadata = file.metadata()?;
        if !metadata.is_dir()
            || metadata.dev() != observation.device
            || metadata.ino() != observation.inode
        {
            return Err("retained descriptor identity changed".into());
        }
        let info = bounded_text(
            &format!("/proc/self/fdinfo/{}", observation.descriptor),
            4096,
        )?;
        let ids = info
            .lines()
            .filter_map(|line| line.strip_prefix("mnt_id:"))
            .map(|value| value.trim().parse::<u64>())
            .collect::<Result<Vec<_>, _>>()?;
        if ids.as_slice() != [observation.mount_id] {
            return Err("retained descriptor mount changed".into());
        }
        let path = std::fs::read_link(format!("/proc/self/fd/{}", observation.descriptor))?;
        let coordinate = path
            .to_str()
            .ok_or(topology::EvidenceError::InvalidCoordinate)?;
        if coordinate.len() > 4096 {
            return Err(topology::EvidenceError::LimitExceeded.into());
        }
        roots[index] = Some(topology::correlate(
            &mounts,
            topology::NativeId {
                device: observation.device,
                inode: observation.inode,
            },
            observation.mount_id,
            coordinate,
        )?);
    }
    let after = bounded_text("/proc/self/mountinfo", 1_048_576)?;
    topology::require_same_snapshot(&before, &after)?;
    let mut matrix = [255; 16];
    for (left, left_root) in roots.iter().enumerate() {
        for (right, right_root) in roots.iter().enumerate() {
            if let (Some(left_root), Some(right_root)) = (left_root, right_root) {
                matrix[left * ROOTS + right] = match topology::relationship(left_root, right_root) {
                    topology::Relationship::Overlapping => 0,
                    topology::Relationship::Separated => 1,
                    topology::Relationship::UnknownFilesystem => 2,
                    topology::Relationship::UnknownDescendantMount => 3,
                };
            }
        }
    }
    Ok(matrix)
}

/// Collects only the four registered fixture roles, without exporting paths.
///
/// # Safety
///
/// `observations` must reference four initialized observations, and `output`
/// must reference sixteen writable bytes. The caller must retain each selected
/// descriptor without closing or reusing it until this function returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn domus_probe_relationships(
    observations: *const Observation,
    mask: u32,
    output: *mut u8,
) -> i32 {
    if observations.is_null() || output.is_null() || mask == 0 || mask & !15 != 0 {
        eprintln!("native relationship proof refused invalid FFI input");
        return -1;
    }
    let observations = unsafe { std::slice::from_raw_parts(observations, ROOTS) };
    match collect(observations, mask) {
        Ok(matrix) => {
            unsafe { std::ptr::copy_nonoverlapping(matrix.as_ptr(), output, matrix.len()) };
            0
        }
        Err(error) => {
            eprintln!("native relationship proof refused evidence: {error}");
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn observation(file: &File) -> Observation {
        let metadata = file.metadata().expect("generated retained directory");
        let info = bounded_text(&format!("/proc/self/fdinfo/{}", file.as_raw_fd()), 4096)
            .expect("retained descriptor metadata");
        let mount_id = info
            .lines()
            .find_map(|line| line.strip_prefix("mnt_id:"))
            .expect("descriptor mount ID")
            .trim()
            .parse()
            .expect("full mount ID");
        Observation {
            descriptor: file.as_raw_fd(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mount_id,
        }
    }

    #[test]
    fn ffi_rejects_changed_identity_without_overwriting_output() {
        let file = File::open("/").expect("retained directory");
        let mut observations = std::array::from_fn::<_, ROOTS, _>(|_| observation(&file));
        observations[0].inode ^= 1;
        let mut output = [99; 16];
        let result =
            unsafe { domus_probe_relationships(observations.as_ptr(), 1, output.as_mut_ptr()) };
        assert_eq!(result, -1);
        assert_eq!(output, [99; 16]);
        assert_eq!(
            file.metadata()
                .expect("caller descriptor remains open")
                .ino(),
            observations[1].inode
        );
    }

    #[test]
    fn ffi_preserves_caller_descriptors_and_unrequested_pairs() {
        let file = File::open("/").expect("retained directory");
        let observations = std::array::from_fn::<_, ROOTS, _>(|_| observation(&file));
        let mut output = [99; 16];
        let result =
            unsafe { domus_probe_relationships(observations.as_ptr(), 1, output.as_mut_ptr()) };
        assert_eq!(result, 0);
        assert_eq!(&output[1..], &[255; 15]);
        assert_eq!(
            file.metadata()
                .expect("caller descriptor remains open")
                .ino(),
            observations[0].inode
        );
    }
}
