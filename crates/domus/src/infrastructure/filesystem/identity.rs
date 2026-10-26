use crate::{Error, Result};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::fs::File;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeRoot {
    pub(crate) identity: String,
    pub(crate) ancestors: Vec<String>,
}

impl NativeRoot {
    pub(super) fn read(directories: &[Dir]) -> Result<Self> {
        let ancestors = directories
            .iter()
            .map(directory_id)
            .collect::<Result<Vec<_>>>()?;
        let identity = ancestors
            .last()
            .ok_or_else(|| Error::Internal("root has no directory handle"))?
            .clone();
        Ok(Self {
            identity,
            ancestors,
        })
    }

    pub(super) fn child(&mut self, directory: &Dir) -> Result<()> {
        self.identity = directory_id(directory)?;
        self.ancestors.push(self.identity.clone());
        Ok(())
    }

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.ancestors.contains(&other.identity) || other.ancestors.contains(&self.identity)
    }
}

fn directory_id(directory: &Dir) -> Result<String> {
    file_id(&directory.try_clone()?.into_std_file())
}

fn file_id(file: &File) -> Result<String> {
    #[cfg(windows)]
    {
        match file_id::get_high_res_file_id_from_file(file)? {
            file_id::FileId::HighRes {
                volume_serial_number,
                file_id,
            } => Ok(format!("win128:{volume_serial_number:016x}:{file_id:032x}")),
            _ => Err(Error::InvalidInput(
                "filesystem did not supply a full Windows file identity".into(),
            )),
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(format!(
            "unix:{:016x}:{:016x}",
            metadata.dev(),
            metadata.ino()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ancestry_detects_alias_overlap_without_confusing_siblings() {
        let parent = NativeRoot {
            identity: "parent".into(),
            ancestors: vec!["volume".into(), "parent".into()],
        };
        let child = NativeRoot {
            identity: "child".into(),
            ancestors: vec!["alias-anchor".into(), "parent".into(), "child".into()],
        };
        let sibling = NativeRoot {
            identity: "sibling".into(),
            ancestors: vec!["volume".into(), "sibling".into()],
        };
        assert!(parent.overlaps(&child));
        assert!(child.overlaps(&parent));
        assert!(parent.overlaps(&parent));
        assert!(!parent.overlaps(&sibling));
        assert!(!child.overlaps(&sibling));
    }
}
