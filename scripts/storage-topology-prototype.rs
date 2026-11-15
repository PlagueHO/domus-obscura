#[path = "../crates/domus/src/infrastructure/filesystem/topology_prototype.rs"]
mod policy;

#[cfg(target_os = "linux")]
mod linux {
    use super::policy::{self, NativeId};
    use std::ffi::OsStr;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Component, Path};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    struct OpenRoot {
        path: String,
        handles: Vec<File>,
        chain: Vec<NativeId>,
        mount_id: u64,
        descriptor_path: String,
    }

    fn native(file: &File) -> io::Result<NativeId> {
        let metadata = file.metadata()?;
        Ok(NativeId {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn bounded_read(path: &str, limit: u64) -> Result<String> {
        let mut bytes = Vec::new();
        File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(io::Error::other("kernel evidence exceeded its bound").into());
        }
        Ok(String::from_utf8(bytes)?)
    }

    fn descriptor(file: &File) -> Result<(u64, String)> {
        let text = bounded_read(&format!("/proc/self/fdinfo/{}", file.as_raw_fd()), 4096)?;
        let mut mount_id = None;
        let mut inode = None;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("mnt_id:") {
                if mount_id.replace(value.trim().parse::<u64>()?).is_some() {
                    return Err(io::Error::other("duplicate descriptor mount ID").into());
                }
            }
            if let Some(value) = line.strip_prefix("ino:") {
                if inode.replace(value.trim().parse::<u64>()?).is_some() {
                    return Err(io::Error::other("duplicate descriptor inode").into());
                }
            }
        }
        if inode != Some(native(file)?.inode) {
            return Err(
                io::Error::other("descriptor evidence does not match the retained handle").into(),
            );
        }
        let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        Ok((
            mount_id.ok_or_else(|| io::Error::other("missing descriptor mount ID"))?,
            path.into_os_string()
                .into_string()
                .map_err(|_| io::Error::other("non-UTF-8 descriptor path"))?,
        ))
    }

    fn open_component(parent: &File, name: &OsStr) -> io::Result<File> {
        let relative = Path::new("/proc/self/fd")
            .join(parent.as_raw_fd().to_string())
            .join(name);
        // This fixed procfs magic link refers to our live descriptor.
        // O_DIRECTORY | O_NOFOLLOW applies to each new component.
        OpenOptions::new()
            .read(true)
            .custom_flags(0o200_000 | 0o400_000)
            .open(relative)
    }

    fn open_root(path: &str) -> Result<OpenRoot> {
        if path.len() > 16_384
            || Path::new(path).components().count() > 256
            || !Path::new(path).is_absolute()
            || path.split('/').any(|part| part == "." || part == "..")
        {
            return Err(
                io::Error::other("an absolute path without dot components is required").into(),
            );
        }
        let mut handles = vec![File::open("/")?];
        for component in Path::new(path).components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let parent = handles
                        .last()
                        .ok_or_else(|| io::Error::other("missing parent handle"))?;
                    handles.push(open_component(parent, name)?);
                }
                _ => return Err(io::Error::other("unsupported path component").into()),
            }
        }
        let final_handle = handles
            .last()
            .ok_or_else(|| io::Error::other("missing root handle"))?;
        let (mount_id, descriptor_path) = descriptor(final_handle)?;
        let chain = handles.iter().map(native).collect::<io::Result<Vec<_>>>()?;
        Ok(OpenRoot {
            path: path.to_owned(),
            handles,
            chain,
            mount_id,
            descriptor_path,
        })
    }

    fn recheck(root: &OpenRoot) -> Result<()> {
        let current = open_root(&root.path)?;
        let retained = root
            .handles
            .last()
            .ok_or_else(|| io::Error::other("missing root handle"))?;
        if current.chain != root.chain
            || current.mount_id != root.mount_id
            || descriptor(retained)? != (root.mount_id, root.descriptor_path.clone())
        {
            return Err(io::Error::other("root path, component identity, or mount changed").into());
        }
        Ok(())
    }

    fn walk_child(root: &OpenRoot, relative: &str) -> Result<File> {
        if relative.is_empty()
            || relative.len() > 16_384
            || Path::new(relative).components().count() > 256
            || relative
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(io::Error::other("a bounded relative child path is required").into());
        }
        let retained = root
            .handles
            .last()
            .ok_or_else(|| io::Error::other("missing root handle"))?;
        let mut children = Vec::new();
        for component in Path::new(relative).components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::other("invalid relative child component").into());
            };
            let parent = children.last().unwrap_or(retained);
            let child = open_component(parent, name)?;
            if descriptor(&child)?.0 != root.mount_id {
                return Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "descendant mount crossing rejected",
                )
                .into());
            }
            children.push(child);
        }
        recheck(root)?;
        children
            .pop()
            .ok_or_else(|| io::Error::other("missing child handle").into())
    }

    fn json_string(text: &str) -> String {
        let mut result = String::from("\"");
        for character in text.chars() {
            match character {
                '"' => result.push_str("\\\""),
                '\\' => result.push_str("\\\\"),
                character if character.is_control() => {
                    result.push_str(&format!("\\u{:04x}", u32::from(character)))
                }
                character => result.push(character),
            }
        }
        result.push('"');
        result
    }

    pub(super) fn run() -> Result<()> {
        let paths: Vec<_> = std::env::args().skip(1).collect();
        if paths.first().is_some_and(|argument| argument == "--child") {
            if paths.len() != 3 {
                return Err(io::Error::other("--child requires a root and relative child").into());
            }
            let child = walk_child(&open_root(&paths[1])?, &paths[2])?;
            let identity = native(&child)?;
            println!(
                "{{\"child_identity\":\"unix:{:016x}:{:016x}\"}}",
                identity.device, identity.inode
            );
            return Ok(());
        }
        if paths.is_empty() || paths.len() > 32 {
            return Err(io::Error::other("supply between one and 32 generated root paths").into());
        }
        let before = bounded_read("/proc/self/mountinfo", 1_048_576)?;
        let mounts = policy::parse_mounts(&before)?;
        let roots = paths
            .iter()
            .map(|path| open_root(path))
            .collect::<Result<Vec<_>>>()?;
        let observed = roots
            .iter()
            .map(|root| {
                let identity = root
                    .chain
                    .last()
                    .copied()
                    .ok_or_else(|| io::Error::other("missing identity"))?;
                Ok(policy::correlate(
                    &mounts,
                    identity,
                    root.mount_id,
                    &root.descriptor_path,
                )?)
            })
            .collect::<Result<Vec<_>>>()?;
        for root in &roots {
            recheck(root)?;
        }
        policy::require_same_snapshot(&before, &bounded_read("/proc/self/mountinfo", 1_048_576)?)?;
        let boundary = roots
            .iter()
            .find(|root| root.path == "/fixture")
            .and_then(|root| root.chain.last());
        let records = roots.iter().map(|root| {
            let identity = root.chain.last().ok_or_else(|| io::Error::other("missing identity"))?;
            let start = boundary.and_then(|boundary| root.chain.iter().position(|id| id == boundary));
            let binding = match start {
                Some(index) => {
                    let chain = root.chain[index..].iter().map(|id| format!("\"unix:{:016x}:{:016x}\"", id.device, id.inode)).collect::<Vec<_>>().join(",");
                    format!("[{chain}]")
                }
                None => "null".to_owned(),
            };
            Ok(format!("{{\"path\":{},\"identity\":\"unix:{:016x}:{:016x}\",\"mount_id\":{},\"candidate_binding\":{}}}", json_string(&root.path), identity.device, identity.inode, root.mount_id, binding))
        }).collect::<Result<Vec<_>>>()?.join(",");
        let mut relationships = Vec::new();
        for (left, first) in observed.iter().enumerate() {
            for (right, second) in observed.iter().enumerate() {
                relationships.push(format!(
                    "{{\"left\":{left},\"right\":{right},\"result\":\"{:?}\"}}",
                    policy::relationship(first, second)
                ));
            }
        }
        println!(
            "{{\"roots\":[{records}],\"relationships\":[{}]}}",
            relationships.join(",")
        );
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);

        #[test]
        fn retained_handle_survives_replacement_but_recheck_rejects_it() -> Result<()> {
            let fixture = std::env::temp_dir().join(format!(
                "domus-topology-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&fixture)?;
            let original = fixture.join("original");
            let moved = fixture.join("moved");
            std::fs::create_dir(&original)?;
            let root = open_root(
                original
                    .to_str()
                    .ok_or_else(|| io::Error::other("invalid fixture path"))?,
            )?;
            recheck(&root)?;
            std::fs::rename(&original, &moved)?;
            std::fs::create_dir(&original)?;
            assert!(recheck(&root).is_err());
            assert_eq!(
                native(
                    root.handles
                        .last()
                        .ok_or_else(|| io::Error::other("missing fixture handle"))?
                )?,
                *root
                    .chain
                    .last()
                    .ok_or_else(|| io::Error::other("missing fixture identity"))?
            );
            std::fs::remove_dir(&original)?;
            std::fs::remove_dir(&moved)?;
            std::fs::remove_dir(&fixture)?;
            Ok(())
        }

        #[test]
        fn symlinks_and_dot_components_are_rejected() -> Result<()> {
            assert!(open_root("/proc/self").is_err());
            assert!(open_root("/tmp/../tmp").is_err());
            assert!(open_root("relative").is_err());
            assert!(open_root("/domus-topology-missing-root").is_err());
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This isolated kernel-evidence probe requires Linux.");
    std::process::exit(2);
}
