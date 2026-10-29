//! Saved library observations describe files, never acquisition or edition proof.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
pub struct LibraryTags {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub album: Option<String>,
    pub date: Option<String>,
    pub track: Option<String>,
    pub disc: Option<String>,
    pub identifiers: std::collections::BTreeMap<String, Vec<String>>,
    pub embedded_cover: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
pub struct LibraryAudio {
    pub format: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bits_per_sample: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
pub struct LibraryMetadata {
    pub tags: LibraryTags,
    pub audio: LibraryAudio,
    pub status: String,
    pub error: Option<String>,
    pub native_identity: Option<String>,
    pub modified_ns: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryArtist {
    pub id: i64,
    pub name: String,
    pub file_count: i64,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryAlbum {
    pub id: i64,
    pub artist_id: i64,
    pub artist: String,
    pub title: String,
    pub folder: String,
    pub file_count: i64,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryFile {
    pub id: i64,
    pub relative_path: String,
    pub present: bool,
    pub scan_id: i64,
    pub observed_at: Option<i64>,
    pub metadata: LibraryMetadata,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryCoverage {
    pub location_id: i64,
    pub coverage: String,
    pub observed_at: Option<i64>,
    pub last_error: Option<String>,
    pub metadata_files: i64,
    pub metadata_issues: i64,
    pub unobserved_files: i64,
    pub folder_state: Option<String>,
    pub folder_observed_at: Option<i64>,
    pub folder_error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct LibraryMatchQuery {
    pub artist: String,
    pub album: Option<String>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryMatch {
    pub query_index: usize,
    pub artist_id: Option<i64>,
    pub albums: Vec<LibraryAlbum>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct LibraryMatches {
    pub coverage: LibraryCoverage,
    pub matches: Vec<LibraryMatch>,
}

pub fn library_key(text: &str) -> String {
    text.trim().to_lowercase()
}
