use super::{AudioFormat, AudioPolicy, AudioProfile, OutputPreview, OutputTemplate, TrackMetadata};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkSize {
    Small,
    Recommended,
    Large,
}

impl ArtworkSize {
    pub fn target_side(self) -> u32 {
        match self {
            Self::Small => 200,
            Self::Recommended => 400,
            Self::Large => 800,
        }
    }
}

/// Administrator-controlled policy for the shared library, not per-user preferences.
/// Plans freeze a settings version so later system changes cannot alter approved work.
/// `format` is preferred; MP3 means verified 320 kbps CBR and FLAC is lossless.
/// A missing accepted set preserves both legacy formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    pub format: AudioFormat,
    #[serde(default)]
    pub accepted_formats: Option<Vec<AudioFormat>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_policy: Option<AudioPolicy>,
    pub template: OutputTemplate,
    #[serde(default)]
    pub artwork_size: Option<ArtworkSize>,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            format: AudioFormat::default(),
            accepted_formats: Some(vec![AudioFormat::Mp3, AudioFormat::Flac]),
            audio_policy: None,
            template: OutputTemplate::default(),
            artwork_size: Some(ArtworkSize::Recommended),
        }
    }
}

impl OutputSettings {
    pub fn policy(&self) -> AudioPolicy {
        self.audio_policy.clone().unwrap_or_else(|| AudioPolicy {
            accepted: self
                .accepted_formats
                .clone()
                .unwrap_or_else(|| vec![AudioFormat::Mp3, AudioFormat::Flac])
                .into_iter()
                .map(AudioProfile::legacy)
                .collect(),
            preferred: AudioProfile::legacy(self.format),
            ..AudioPolicy::default()
        })
    }

    pub fn accepts(&self, format: AudioFormat) -> bool {
        self.accepted_formats
            .as_ref()
            .is_none_or(|formats| formats.contains(&format))
    }

    pub fn validate(&self) -> Result<()> {
        let policy = self.policy();
        policy.validate()?;
        if self.audio_policy.is_some()
            && (policy.preferred.format() != self.format
                || self.accepted_formats.as_ref().is_none_or(|formats| {
                    let expected = policy.formats();
                    formats.len() != expected.len()
                        || formats.iter().any(|format| !expected.contains(format))
                }))
        {
            return Err(Error::InvalidInput(
                "legacy audio format fields must agree with the explicit audio policy".into(),
            ));
        }
        if let Some(formats) = &self.accepted_formats
            && (formats.is_empty()
                || formats.len() > 2
                || !formats.contains(&self.format)
                || formats
                    .iter()
                    .enumerate()
                    .any(|(index, format)| formats[..index].contains(format)))
        {
            return Err(Error::InvalidInput(
                "select distinct accepted audio formats and a preferred format from that selection"
                    .into(),
            ));
        }
        self.template.preview(
            &TrackMetadata {
                album_artist: "Artist",
                track_artist: "Artist",
                album: "Album",
                title: "Title",
                track: 1,
                track_total: 1,
                disc: Some(1),
                disc_track: Some(1),
                bucket: None,
            },
            self.format,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct PlannedTrack {
    pub artist: String,
    pub title: String,
    pub disc: Option<u16>,
    pub disc_track: Option<u16>,
}

/// An exact MusicBrainz release, not a release group or a peer's folder name.
/// The ordered tracks define continuous output numbering across all discs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct EditionSelection {
    pub release_id: String,
    pub album_artist: String,
    pub album: String,
    pub bucket: Option<String>,
    pub tracks: Vec<PlannedTrack>,
}

impl EditionSelection {
    pub fn preview(&self, settings: &OutputSettings) -> Result<Vec<OutputPreview>> {
        if !super::identifiers::is_canonical_uuid(&self.release_id) {
            return Err(Error::InvalidInput(
                "release ID must be a canonical lowercase MusicBrainz UUID".into(),
            ));
        }
        if self.tracks.is_empty() || self.tracks.len() > 1000 {
            return Err(Error::InvalidInput(
                "a plan must contain 1 through 1000 ordered tracks".into(),
            ));
        }
        let total = u32::try_from(self.tracks.len())
            .map_err(|_| Error::InvalidInput("track count exceeds the supported range".into()))?;
        let mut filenames = HashSet::new();
        let mut previews = Vec::with_capacity(self.tracks.len());
        for (index, track) in self.tracks.iter().enumerate() {
            let number = u32::try_from(index + 1).map_err(|_| {
                Error::InvalidInput("track number exceeds the supported range".into())
            })?;
            let preview = settings.template.preview(
                &TrackMetadata {
                    album_artist: &self.album_artist,
                    album: &self.album,
                    track_artist: &track.artist,
                    title: &track.title,
                    track: number,
                    track_total: total,
                    disc: track.disc,
                    disc_track: track.disc_track,
                    bucket: self.bucket.as_deref(),
                },
                settings.format,
            )?;
            if previews.first().is_some_and(|first: &OutputPreview| {
                first.relative_folder != preview.relative_folder
            }) {
                return Err(Error::InvalidInput(
                    "all tracks of one album must use the same output folder".into(),
                ));
            }
            if !filenames.insert(preview.filename.to_uppercase()) {
                return Err(Error::InvalidInput(
                    "output filenames collide under portable case-insensitive comparison".into(),
                ));
            }
            previews.push(preview);
        }
        Ok(previews)
    }
}
