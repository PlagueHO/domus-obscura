use crate::{Error, Result};
use serde::Serialize;

/// A portable relative identifier, never an operating-system absolute path.
/// Validate before converting to a native path in the filesystem adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LibraryPath(String);

impl LibraryPath {
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() > 4096
            || value.contains(['\\', ':', '\0'])
            || value.chars().any(char::is_control)
            || (!value.is_empty()
                && value
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == ".."))
        {
            return Err(Error::InvalidInput(
                "library path must contain only relative, slash-separated components".into(),
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(parent, _)| parent)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Page {
    after: i64,
    limit: u16,
}

impl Page {
    pub fn after(self) -> i64 {
        self.after
    }

    pub fn limit(self) -> u16 {
        self.limit
    }

    pub fn new(after: i64, limit: u16) -> Result<Self> {
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(Error::InvalidInput(
                "page cursor must be nonnegative and page size must be 1 through 200".into(),
            ));
        }
        Ok(Self { after, limit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escapes_and_alternate_path_syntax() {
        for value in [
            "../secret",
            "a/../secret",
            "/secret",
            "C:/secret",
            "a\\secret",
            "file:stream",
            "a//b",
            "./file",
            "a/\0",
        ] {
            assert!(LibraryPath::parse(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn accepts_root_and_unicode_names() -> Result<()> {
        assert_eq!(LibraryPath::parse("")?.as_str(), "");
        assert_eq!(
            LibraryPath::parse("A/ÄRTIST/album.flac")?.parent(),
            "A/ÄRTIST"
        );
        Ok(())
    }

    #[test]
    fn page_bounds_are_enforced() {
        assert!(Page::new(0, 1).is_ok());
        assert!(Page::new(0, 200).is_ok());
        assert!(Page::new(0, 201).is_err());
        assert!(Page::new(-1, 20).is_err());
    }
}
