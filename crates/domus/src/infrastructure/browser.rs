//! Immutable, bounded browser assets loaded before the listener starts. Request
//! paths select map keys only; they never enter filesystem APIs.
use super::local_file::read_local;
use crate::{Error, Result};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::path::Path;

pub struct BrowserAssets {
    files: HashMap<String, (&'static str, Vec<u8>)>,
}

impl BrowserAssets {
    pub fn load(root: &Path) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::InvalidInput(
                "browser asset directory must be absolute".into(),
            ));
        }
        let mut files = HashMap::new();
        let index = read_local(&root.join("index.html"), 65_536, false)?;
        let mut total = index.len();
        files.insert("/".into(), ("text/html; charset=utf-8", index));
        for entry in std::fs::read_dir(root.join("assets"))? {
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
            let mime = match entry.path().extension().and_then(|s| s.to_str()) {
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
            let bytes = read_local(&entry.path(), 8 * 1024 * 1024, false)?;
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
