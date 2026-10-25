//! Peer paths are protocol identifiers, never native filesystem paths.
//! A complete listing and matching names do not prove encoding or exact edition.
use super::{AudioFormat, EditionSelection};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_FOLDER_FILES: usize = 1000;
pub const MAX_FOLDER_DIRECTORIES: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SourceFileMetadata {
    pub remote_path: String,
    pub expected_bytes: i64,
    pub extension: String,
    pub bitrate_kbps: Option<u32>,
    pub duration_seconds: Option<u32>,
    pub vbr: Option<bool>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SourceFolder {
    pub peer: String,
    pub folder: String,
    pub directories: u16,
    pub files: Vec<SourceFileMetadata>,
}

/// These limits apply to remote identifiers only. Do not pass the result to
/// std::path, LibraryPath, or any filesystem API.
pub fn remote_identifier(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput("invalid peer path identifier".into()));
    }
    let canonical = value.replace('/', "\\");
    if canonical
        .split('\\')
        .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Error::InvalidInput("invalid peer path components".into()));
    }
    Ok(canonical)
}

impl SourceFolder {
    pub fn validate(&self) -> Result<()> {
        if self.peer.is_empty()
            || self.peer.len() > 30
            || self.peer.trim() != self.peer
            || !self.peer.bytes().all(|byte| (32..=126).contains(&byte))
            || self.directories == 0
            || usize::from(self.directories) > MAX_FOLDER_DIRECTORIES
            || self.files.len() > MAX_FOLDER_FILES
        {
            return Err(Error::InvalidInput(
                "invalid bounded peer folder listing".into(),
            ));
        }
        let folder = remote_identifier(&self.folder)?;
        let prefix = format!("{folder}\\");
        let mut paths = HashSet::new();
        for file in &self.files {
            let path = remote_identifier(&file.remote_path)?;
            if !path.starts_with(&prefix)
                || !paths.insert(path.to_uppercase())
                || file.expected_bytes <= 0
                || file.extension.len() > 16
                || !file
                    .extension
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric())
            {
                return Err(Error::InvalidInput(
                    "peer listing has invalid, duplicate or out-of-folder files".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceMappingIssue {
    MissingTrack,
    AmbiguousTrack,
    UnexpectedAudio,
    UnsupportedFile,
    AdvertisedQualityMismatch,
    AdvertisedQualityUnknown,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SourceTrackMatch {
    pub track_number: u16,
    pub remote_path: String,
    pub expected_bytes: i64,
    pub rule: String,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SourceMappingProblem {
    pub issue: SourceMappingIssue,
    pub track_number: Option<u16>,
    pub remote_path: Option<String>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SourceAssessment {
    pub release_id: String,
    pub peer: String,
    pub folder: String,
    pub format: AudioFormat,
    pub tracks: Vec<SourceTrackMatch>,
    pub problems: Vec<SourceMappingProblem>,
    pub complete_mapping: bool,
    pub advertised_quality_matches: bool,
    pub edition_verified: bool,
    pub audio_verified: bool,
    pub requires_user_review: bool,
}

fn text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn named(stem: &str, keys: &[String; 2]) -> bool {
    keys.iter().any(|key| key == stem)
}

fn match_rule(stem: &str, keys: &[String; 2], index: usize) -> Option<&'static str> {
    // Numeric prefixes are evidence, not decoration. A conflicting prefix cannot
    // be stripped and reinterpreted as another track.
    if named(stem, keys) {
        return Some("exact_title_or_artist_title");
    }
    let digits = stem.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number = stem[..digits].parse::<usize>().ok()?;
    if number != index + 1 {
        return None;
    }
    let rest = &stem[digits..];
    let rest = rest
        .strip_prefix(" - ")
        .or_else(|| rest.strip_prefix(". "))
        .or_else(|| rest.strip_prefix(' '))?;
    named(rest, keys).then_some("continuous_number_and_exact_title_or_artist_title")
}

fn extension(file: &SourceFileMetadata) -> Option<String> {
    let path = file.remote_path.rsplit(['\\', '/']).next()?;
    let (_, suffix) = path.rsplit_once('.')?;
    if !file.extension.is_empty() && !file.extension.eq_ignore_ascii_case(suffix) {
        return None;
    }
    Some(suffix.to_ascii_lowercase())
}

/// Map only a complete single-peer folder. No fuzzy title matching, format
/// substitution, extra-audio filtering, approval, or automatic queue effects.
/// Advertised attributes are not a substitute for complete audio inspection.
pub fn assess_source(
    edition: &EditionSelection,
    format: AudioFormat,
    listing: &SourceFolder,
) -> Result<SourceAssessment> {
    if !super::identifiers::is_canonical_uuid(&edition.release_id)
        || edition.tracks.is_empty()
        || edition.tracks.len() > 1000
        || edition.tracks.iter().any(|track| {
            [&track.title, &track.artist].iter().any(|value| {
                value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
            })
        })
    {
        return Err(Error::InvalidInput(
            "source assessment needs a bounded exact-release track list".into(),
        ));
    }
    listing.validate()?;
    let mut tracks = Vec::new();
    let mut problems = Vec::new();
    let keys = edition
        .tracks
        .iter()
        .map(|track| {
            [
                text(&track.title),
                text(&format!("{} - {}", track.artist, track.title)),
            ]
        })
        .collect::<Vec<_>>();
    let mut candidates = vec![Vec::new(); edition.tracks.len()];
    for (file_index, file) in listing.files.iter().enumerate() {
        let ext = extension(file);
        let supported_asset = matches!(
            ext.as_deref(),
            Some("jpg" | "jpeg" | "png" | "pdf" | "txt" | "log" | "cue" | "m3u" | "m3u8")
        );
        if ext.as_deref() != Some(format.extension()) {
            if !supported_asset {
                problems.push(SourceMappingProblem {
                    issue: if matches!(
                        ext.as_deref(),
                        Some(
                            "mp3"
                                | "flac"
                                | "wav"
                                | "ogg"
                                | "opus"
                                | "m4a"
                                | "aac"
                                | "wma"
                                | "ape"
                                | "aiff"
                                | "alac"
                                | "dsf"
                                | "dff"
                        )
                    ) {
                        SourceMappingIssue::UnexpectedAudio
                    } else {
                        SourceMappingIssue::UnsupportedFile
                    },
                    track_number: None,
                    remote_path: Some(file.remote_path.clone()),
                });
            }
            continue;
        }
        let name = file
            .remote_path
            .rsplit(['\\', '/'])
            .next()
            .ok_or_else(|| Error::InvalidInput("missing peer filename".into()))?;
        let stem = name
            .rsplit_once('.')
            .ok_or_else(|| Error::InvalidInput("missing peer filename extension".into()))?
            .0;
        let stem = text(stem);
        let mut found = false;
        for (index, track_keys) in keys.iter().enumerate() {
            if let Some(rule) = match_rule(&stem, track_keys, index) {
                candidates[index].push((file_index, rule));
                found = true;
            }
        }
        if !found {
            problems.push(SourceMappingProblem {
                issue: SourceMappingIssue::UnexpectedAudio,
                track_number: None,
                remote_path: Some(file.remote_path.clone()),
            });
        }
    }
    // A file matching repeated catalog titles is ambiguous even if each title
    // has only that file. Never use one file for two release tracks.
    let mut use_counts = vec![0usize; listing.files.len()];
    for matches in &candidates {
        for &(file, _) in matches {
            use_counts[file] += 1;
        }
    }
    for (index, matches) in candidates.iter().enumerate() {
        let number = u16::try_from(index + 1)
            .map_err(|_| Error::InvalidInput("too many source tracks".into()))?;
        if let [(file_index, rule)] = matches.as_slice()
            && use_counts[*file_index] == 1
        {
            let file = &listing.files[*file_index];
            tracks.push(SourceTrackMatch {
                track_number: number,
                remote_path: file.remote_path.clone(),
                expected_bytes: file.expected_bytes,
                rule: (*rule).into(),
            });
            let issue = if format == AudioFormat::Mp3 {
                if file.bitrate_kbps.is_some_and(|value| value != 320) || file.vbr == Some(true) {
                    Some(SourceMappingIssue::AdvertisedQualityMismatch)
                } else if file.bitrate_kbps != Some(320) || file.vbr != Some(false) {
                    Some(SourceMappingIssue::AdvertisedQualityUnknown)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(issue) = issue {
                problems.push(SourceMappingProblem {
                    issue,
                    track_number: Some(number),
                    remote_path: Some(file.remote_path.clone()),
                });
            }
        } else {
            problems.push(SourceMappingProblem {
                issue: if matches.is_empty() {
                    SourceMappingIssue::MissingTrack
                } else {
                    SourceMappingIssue::AmbiguousTrack
                },
                track_number: Some(number),
                remote_path: None,
            });
        }
    }
    let complete_mapping = tracks.len() == edition.tracks.len()
        && problems.iter().all(|problem| {
            matches!(
                problem.issue,
                SourceMappingIssue::AdvertisedQualityMismatch
                    | SourceMappingIssue::AdvertisedQualityUnknown
            )
        });
    let advertised_quality_matches = complete_mapping
        && !problems.iter().any(|problem| {
            matches!(
                problem.issue,
                SourceMappingIssue::AdvertisedQualityMismatch
                    | SourceMappingIssue::AdvertisedQualityUnknown
            )
        });
    Ok(SourceAssessment {
        release_id: edition.release_id.clone(),
        peer: listing.peer.clone(),
        folder: listing.folder.clone(),
        format,
        tracks,
        problems,
        complete_mapping,
        advertised_quality_matches,
        edition_verified: false,
        audio_verified: false,
        requires_user_review: true,
    })
}
