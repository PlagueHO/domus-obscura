use super::{AudioFormat, OutputPreview, OutputTemplate, TrackMetadata};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    pub format: AudioFormat,
    pub template: OutputTemplate,
}

impl OutputSettings {
    pub fn validate(&self) -> Result<()> {
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
