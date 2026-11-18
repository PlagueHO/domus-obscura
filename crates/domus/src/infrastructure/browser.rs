//! Immutable, bounded browser assets loaded before the listener starts. Request
//! paths select map keys only; they never enter filesystem APIs.
use super::filesystem::{FilesystemClient, LibraryDirectory, read_browser_file_in};
use crate::{Error, Result};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub struct BrowserAssets {
    files: HashMap<String, (&'static str, Vec<u8>)>,
}

impl BrowserAssets {
    pub fn load(root: &Path) -> Result<Self> {
        Self::load_for(root, None)
    }

    pub(crate) fn load_for(
        root: &Path,
        filesystem: Option<&Arc<FilesystemClient>>,
    ) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::InvalidInput(
                "browser asset directory must be absolute".into(),
            ));
        }
        let directory = LibraryDirectory::open(root)?;
        if let Some(filesystem) = filesystem {
            filesystem.require_non_media_input(&directory)?;
        }
        let mut files = HashMap::new();
        let index = read_browser_file_in(&directory, std::ffi::OsStr::new("index.html"), 65_536)?;
        let mut total = index.len();
        files.insert("/".into(), ("text/html; charset=utf-8", index));
        let assets = directory.open_directory(&crate::domain::LibraryPath::parse("assets")?)?;
        if let Some(filesystem) = filesystem {
            filesystem.require_non_media_input(&assets)?;
        }
        for entry in assets.entries()? {
            let entry = entry?;
            if files.len() >= 128 || !entry.file_type()?.is_file() {
                return Err(Error::InvalidInput(
                    "browser assets must be at most 127 regular files".into(),
                ));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::InvalidInput("browser asset name is not UTF-8".into()))?;
            if !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(Error::InvalidInput("browser asset name is invalid".into()));
            }
            let mime = match Path::new(&name).extension().and_then(|s| s.to_str()) {
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("ttf") => "font/ttf",
                Some("woff2") => "font/woff2",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                _ => {
                    return Err(Error::InvalidInput(
                        "browser asset type is unsupported".into(),
                    ));
                }
            };
            let bytes =
                read_browser_file_in(&assets, std::ffi::OsStr::new(&name), 8 * 1024 * 1024)?;
            total += bytes.len();
            if total > 16 * 1024 * 1024 {
                return Err(Error::InvalidInput("browser assets exceed 16 MiB".into()));
            }
            files.insert(format!("/assets/{name}"), (mime, bytes));
        }
        Ok(Self { files })
    }

    pub fn response(&self, path: &str) -> Response {
        match self.files.get(path) {
            Some((mime, bytes)) => ([(header::CONTENT_TYPE, *mime)], bytes.clone()).into_response(),
            None => (StatusCode::NOT_FOUND, "browser asset not found").into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_browser_assets_refuse_a_media_child_before_reading_it() -> Result<()> {
        let fixture = super::super::permissions::private_test_directory()?;
        let application = fixture.path().join("application");
        let browser = fixture.path().join("browser");
        let assets = browser.join("assets");
        std::fs::create_dir(&application)?;
        super::super::permissions::private_fixture::directory(&application)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::create_dir(&browser)?;
        std::fs::create_dir(&assets)?;
        std::fs::write(browser.join("index.html"), b"generated browser")?;
        std::fs::write(assets.join("app.js"), b"generated asset")?;
        drop(super::super::store::Store::open(
            &application.join("state.db"),
        )?);
        let storage = super::super::filesystem::fixture_storage(
            &application,
            &crate::domain::LibraryPath::parse("")?,
            &[(
                "media",
                super::super::filesystem::StorageRole::Media,
                assets.as_path(),
            )],
        )?;
        let client = FilesystemClient::new(storage, "a".repeat(64))?;
        assert!(BrowserAssets::load(&browser).is_ok());
        assert!(matches!(
            BrowserAssets::load_for(&browser, Some(&client)),
            Err(Error::Conflict(_))
        ));
        assert_eq!(std::fs::read(assets.join("app.js"))?, b"generated asset");
        Ok(())
    }
}
