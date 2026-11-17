//! Test-only relationship model, also compiled by the isolated Linux probe.
//!
//! Kernel mount coordinates are transient evidence, not saved root authority.
//! Only roots on the same ext4 device with bounded ASCII coordinates and no
//! descendant mounts can receive a separated result from this experiment.

use std::fmt;

const MAX_SNAPSHOT_BYTES: usize = 1_048_576;
const MAX_MOUNTS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeId {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvidenceError {
    LimitExceeded,
    MalformedMountInfo,
    InvalidCoordinate,
    MissingMount,
    DeviceMismatch,
    TopologyChanged,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for EvidenceError {}

#[derive(Debug)]
pub(crate) struct Mount {
    id: u64,
    device: (u32, u32),
    root: String,
    point: String,
    filesystem: String,
}

#[derive(Debug)]
pub(crate) struct Root {
    native: NativeId,
    coordinate: String,
    supported_filesystem: bool,
    descendant_mount: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Relationship {
    Overlapping,
    Separated,
    UnknownFilesystem,
    UnknownDescendantMount,
}

fn decode_field(field: &str) -> Result<String, EvidenceError> {
    let mut decoded = String::new();
    let mut bytes = field.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            let escape = [bytes.next(), bytes.next(), bytes.next()];
            decoded.push(match escape {
                [Some(b'0'), Some(b'4'), Some(b'0')] => ' ',
                [Some(b'0'), Some(b'1'), Some(b'1')] => '\t',
                [Some(b'0'), Some(b'1'), Some(b'2')] => '\n',
                [Some(b'1'), Some(b'3'), Some(b'4')] => '\\',
                _ => return Err(EvidenceError::MalformedMountInfo),
            });
        } else if byte.is_ascii() {
            decoded.push(char::from(byte));
        } else {
            return Err(EvidenceError::InvalidCoordinate);
        }
    }
    Ok(decoded)
}

fn validate_coordinate(path: &str) -> Result<(), EvidenceError> {
    if !path.starts_with('/')
        || !path.is_ascii()
        || path.bytes().any(|byte| byte.is_ascii_control())
        || path.ends_with(" (deleted)")
        || (path != "/"
            && path
                .split('/')
                .skip(1)
                .any(|part| matches!(part, "" | "." | "..")))
    {
        return Err(EvidenceError::InvalidCoordinate);
    }
    Ok(())
}

pub(crate) fn parse_mounts(snapshot: &str) -> Result<Vec<Mount>, EvidenceError> {
    if snapshot.len() > MAX_SNAPSHOT_BYTES {
        return Err(EvidenceError::LimitExceeded);
    }
    let mut mounts = Vec::new();
    for line in snapshot.lines() {
        if mounts.len() == MAX_MOUNTS || line.len() > 16_384 {
            return Err(EvidenceError::LimitExceeded);
        }
        let (before, after) = line
            .split_once(" - ")
            .ok_or(EvidenceError::MalformedMountInfo)?;
        let fields: Vec<_> = before.split_ascii_whitespace().collect();
        let trailing: Vec<_> = after.split_ascii_whitespace().collect();
        if fields.len() < 6 || fields.len() > 64 || trailing.len() != 3 {
            return Err(EvidenceError::MalformedMountInfo);
        }
        let id = fields[0]
            .parse::<u64>()
            .map_err(|_| EvidenceError::MalformedMountInfo)?;
        let _parent = fields[1]
            .parse::<u64>()
            .map_err(|_| EvidenceError::MalformedMountInfo)?;
        let (major, minor) = fields[2]
            .split_once(':')
            .ok_or(EvidenceError::MalformedMountInfo)?;
        let device = (
            major
                .parse::<u32>()
                .map_err(|_| EvidenceError::MalformedMountInfo)?,
            minor
                .parse::<u32>()
                .map_err(|_| EvidenceError::MalformedMountInfo)?,
        );
        if id == 0 || mounts.iter().any(|mount: &Mount| mount.id == id) {
            return Err(EvidenceError::MalformedMountInfo);
        }
        let root = decode_field(fields[3])?;
        let point = decode_field(fields[4])?;
        validate_coordinate(&root)?;
        validate_coordinate(&point)?;
        mounts.push(Mount {
            id,
            device,
            root,
            point,
            filesystem: trailing[0].to_owned(),
        });
    }
    if mounts.is_empty() {
        return Err(EvidenceError::MalformedMountInfo);
    }
    Ok(mounts)
}

fn within(path: &str, parent: &str, ignore_ascii_case: bool) -> bool {
    if parent == "/" {
        return true;
    }
    let mut path_parts = path.split('/');
    parent.split('/').all(|parent_part| {
        path_parts.next().is_some_and(|path_part| {
            if ignore_ascii_case {
                path_part.eq_ignore_ascii_case(parent_part)
            } else {
                path_part == parent_part
            }
        })
    })
}

pub(crate) fn correlate(
    mounts: &[Mount],
    native: NativeId,
    mount_id: u64,
    descriptor_path: &str,
) -> Result<Root, EvidenceError> {
    validate_coordinate(descriptor_path)?;
    let mount = mounts
        .iter()
        .find(|mount| mount.id == mount_id)
        .ok_or(EvidenceError::MissingMount)?;
    let device = (
        (((native.device >> 8) & 0xfff) | ((native.device >> 32) & 0xffff_f000)) as u32,
        ((native.device & 0xff) | ((native.device >> 12) & 0xffff_ff00)) as u32,
    );
    if mount.device != device {
        return Err(EvidenceError::DeviceMismatch);
    }
    if !within(descriptor_path, &mount.point, false) {
        return Err(EvidenceError::InvalidCoordinate);
    }
    let suffix = if mount.point == "/" {
        descriptor_path
    } else {
        &descriptor_path[mount.point.len()..]
    };
    let coordinate = format!("{}{}", mount.root.trim_end_matches('/'), suffix);
    let coordinate = if coordinate.is_empty() {
        "/".to_owned()
    } else {
        coordinate
    };
    validate_coordinate(&coordinate)?;
    Ok(Root {
        native,
        coordinate,
        supported_filesystem: mount.filesystem == "ext4",
        descendant_mount: mounts
            .iter()
            .any(|other| other.id != mount.id && within(&other.point, descriptor_path, true)),
    })
}

pub(crate) fn require_same_snapshot(before: &str, after: &str) -> Result<(), EvidenceError> {
    if before != after {
        return Err(EvidenceError::TopologyChanged);
    }
    Ok(())
}

pub(crate) fn relationship(left: &Root, right: &Root) -> Relationship {
    if left.native == right.native {
        return Relationship::Overlapping;
    }
    if !left.supported_filesystem
        || !right.supported_filesystem
        || left.native.device != right.native.device
    {
        return Relationship::UnknownFilesystem;
    }
    if within(&left.coordinate, &right.coordinate, true)
        || within(&right.coordinate, &left.coordinate, true)
    {
        return Relationship::Overlapping;
    }
    if left.descendant_mount || right.descendant_mount {
        return Relationship::UnknownDescendantMount;
    }
    Relationship::Separated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(snapshot: &str, mount: u64, inode: u64, path: &str) -> Result<Root, EvidenceError> {
        correlate(
            &parse_mounts(snapshot)?,
            NativeId {
                device: 2049,
                inode,
            },
            mount,
            path,
        )
    }

    const ALIASES: &str = "\
11 1 8:1 /payload /media rw - ext4 /dev/example rw\n\
12 1 8:1 /payload/staging /private rw - ext4 /dev/example rw\n\
13 1 8:1 /sibling /sibling rw - ext4 /dev/example rw\n";

    #[test]
    fn hidden_nested_aliases_overlap_in_both_directions() -> Result<(), EvidenceError> {
        let media = observed(ALIASES, 11, 100, "/media")?;
        let staging = observed(ALIASES, 12, 200, "/private")?;
        assert_eq!(relationship(&media, &staging), Relationship::Overlapping);
        assert_eq!(relationship(&staging, &media), Relationship::Overlapping);
        let through_media = observed(ALIASES, 11, 200, "/media/staging")?;
        assert_eq!(
            relationship(&through_media, &staging),
            Relationship::Overlapping
        );
        Ok(())
    }

    #[test]
    fn siblings_and_prefix_lookalikes_are_separated() -> Result<(), EvidenceError> {
        let snapshot =
            format!("{ALIASES}14 1 8:1 /payload-extra /extra rw - ext4 /dev/example rw\n");
        let media = observed(&snapshot, 11, 100, "/media")?;
        let sibling = observed(&snapshot, 13, 300, "/sibling")?;
        let lookalike = observed(&snapshot, 14, 400, "/extra")?;
        assert_eq!(relationship(&media, &sibling), Relationship::Separated);
        assert_eq!(relationship(&media, &lookalike), Relationship::Separated);
        Ok(())
    }

    #[test]
    fn case_variants_cannot_create_a_false_separation() -> Result<(), EvidenceError> {
        let snapshot = ALIASES.replace("/payload/staging", "/PAYLOAD/staging");
        let media = observed(&snapshot, 11, 100, "/media")?;
        let staging = observed(&snapshot, 12, 200, "/private")?;
        assert_eq!(relationship(&media, &staging), Relationship::Overlapping);
        Ok(())
    }

    #[test]
    fn descendant_mounts_and_opaque_filesystems_are_unknown() -> Result<(), EvidenceError> {
        let snapshot =
            format!("{ALIASES}14 11 8:1 /sibling /media/injected rw - ext4 /dev/example rw\n");
        let media = observed(&snapshot, 11, 100, "/media")?;
        let sibling = observed(&snapshot, 13, 300, "/sibling")?;
        assert_eq!(
            relationship(&media, &sibling),
            Relationship::UnknownDescendantMount
        );
        let opaque = observed(&ALIASES.replace("ext4", "cifs"), 11, 100, "/media")?;
        assert_eq!(
            relationship(&opaque, &sibling),
            Relationship::UnknownFilesystem
        );
        Ok(())
    }

    #[test]
    fn different_devices_do_not_prove_separation() -> Result<(), EvidenceError> {
        let snapshot = format!("{ALIASES}14 1 8:2 /other /other rw - ext4 /dev/example2 rw\n");
        let media = observed(&snapshot, 11, 100, "/media")?;
        let other = correlate(
            &parse_mounts(&snapshot)?,
            NativeId {
                device: 2050,
                inode: 100,
            },
            14,
            "/other",
        )?;
        assert_eq!(
            relationship(&media, &other),
            Relationship::UnknownFilesystem
        );
        Ok(())
    }

    #[test]
    fn missing_mismatched_and_changed_evidence_is_rejected() -> Result<(), EvidenceError> {
        assert!(matches!(
            observed(ALIASES, 99, 100, "/media"),
            Err(EvidenceError::MissingMount)
        ));
        assert!(matches!(
            correlate(
                &parse_mounts(ALIASES)?,
                NativeId {
                    device: 2050,
                    inode: 100
                },
                11,
                "/media"
            ),
            Err(EvidenceError::DeviceMismatch)
        ));
        assert!(matches!(
            observed(ALIASES, 11, 100, "/unrelated"),
            Err(EvidenceError::InvalidCoordinate)
        ));
        assert_eq!(
            require_same_snapshot(ALIASES, &ALIASES.replace("11 1", "99 1")),
            Err(EvidenceError::TopologyChanged)
        );
        assert_eq!(
            require_same_snapshot(ALIASES, &ALIASES.replace("/payload ", "/replacement ")),
            Err(EvidenceError::TopologyChanged)
        );
        require_same_snapshot(ALIASES, ALIASES)
    }

    #[test]
    fn malformed_ambiguous_and_unbounded_mount_text_is_rejected() {
        for snapshot in [
            "",
            "11 1 8:1 / /media rw ext4 /dev/example rw\n",
            "11 1 invalid / /media rw - ext4 /dev/example rw\n",
            "11 1 8:1 / /media rw - ext4 /dev/example\n",
            "11 1 8:1 / /media rw - ext4 /dev/example rw extra\n",
            "11 1 8:1 / /media rw - ext4 /dev/example rw\n11 1 8:1 / /other rw - ext4 /dev/example rw\n",
            "11 1 8:1 /../escape /media rw - ext4 /dev/example rw\n",
            "11 1 8:1 /bad\\999 /media rw - ext4 /dev/example rw\n",
            "11 1 8:1 /non-ascii-\u{e9} /media rw - ext4 /dev/example rw\n",
        ] {
            assert!(parse_mounts(snapshot).is_err(), "{snapshot}");
        }
        assert!(matches!(
            parse_mounts(&"x".repeat(MAX_SNAPSHOT_BYTES + 1)),
            Err(EvidenceError::LimitExceeded)
        ));
        let excessive = (1..=MAX_MOUNTS + 1)
            .map(|id| format!("{id} 1 8:1 / /media rw - ext4 /dev/example rw\n"))
            .collect::<String>();
        assert!(matches!(
            parse_mounts(&excessive),
            Err(EvidenceError::LimitExceeded)
        ));
    }

    #[test]
    fn escaped_fields_and_mountpoint_root_projection_are_handled() -> Result<(), EvidenceError> {
        let snapshot =
            "11 1 8:1 /my\\040root / rw shared:2 future-option:value - ext4 /dev/example rw\n";
        let child = observed(snapshot, 11, 100, "/child")?;
        assert_eq!(child.coordinate, "/my root/child");
        let escaped = parse_mounts("11 1 8:1 /back\\134slash /media rw - ext4 /dev/example rw\n")?;
        assert_eq!(escaped[0].root, "/back\\slash");
        Ok(())
    }
}
