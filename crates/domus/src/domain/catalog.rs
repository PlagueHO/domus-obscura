use super::{EditionSelection, PlannedTrack};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogPage<T> {
    pub offset: u32,
    pub count: u32,
    pub items: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogArtist {
    pub id: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub disambiguation: Option<String>,
    pub country: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogCredit {
    pub artist_id: String,
    pub name: String,
    pub joinphrase: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogReleaseSummary {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub status: Option<String>,
    pub group: Option<CatalogReleaseGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogReleaseGroup {
    pub id: String,
    pub primary_type: Option<String>,
    pub secondary_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogReleaseFilters {
    pub include_singles: bool,
    pub include_live: bool,
    pub include_compilations: bool,
    pub include_unofficial: bool,
}

impl CatalogReleaseFilters {
    pub fn includes(&self, release: &CatalogReleaseSummary) -> bool {
        let Some(group) = &release.group else {
            return false;
        };
        let primary = group.primary_type.as_deref().is_some_and(|kind| {
            kind.eq_ignore_ascii_case("album")
                || kind.eq_ignore_ascii_case("ep")
                || (self.include_singles && kind.eq_ignore_ascii_case("single"))
        });
        primary
            && (self.include_unofficial
                || release
                    .status
                    .as_deref()
                    .is_some_and(|status| status.eq_ignore_ascii_case("official")))
            && (self.include_live
                || !group
                    .secondary_types
                    .iter()
                    .any(|kind| kind.eq_ignore_ascii_case("live")))
            && (self.include_compilations
                || !group
                    .secondary_types
                    .iter()
                    .any(|kind| kind.eq_ignore_ascii_case("compilation")))
    }
}

/// Counts and cursors refer to provider rows before local policy filtering.
/// A page can have no visible releases and still have a next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogReleasePage {
    pub offset: u32,
    pub count: u32,
    pub scanned: u16,
    pub next_offset: Option<u32>,
    pub filters: CatalogReleaseFilters,
    pub items: Vec<CatalogReleaseSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogTrack {
    pub track_id: String,
    pub recording_id: String,
    pub position: u16,
    pub number: String,
    pub title: String,
    pub credits: Vec<CatalogCredit>,
    pub duration_ms: Option<u32>,
    pub video: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogMedium {
    pub position: u16,
    pub format: Option<String>,
    pub title: Option<String>,
    pub has_pregap: bool,
    pub has_data_tracks: bool,
    pub tracks: Vec<CatalogTrack>,
}

/// Release and recording identifiers remain separate. A release group or a
/// folder label cannot substitute for this exact edition and ordered mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogRelease {
    pub summary: CatalogReleaseSummary,
    pub release_group_id: String,
    pub credits: Vec<CatalogCredit>,
    pub media: Vec<CatalogMedium>,
}

impl CatalogRelease {
    pub fn selection(&self, bucket: Option<String>) -> Result<EditionSelection> {
        if self
            .media
            .iter()
            .any(|medium| medium.has_pregap || medium.has_data_tracks)
        {
            return Err(Error::Conflict(
                "pregap or data tracks require an explicit inclusion decision".into(),
            ));
        }
        if self
            .media
            .iter()
            .flat_map(|medium| &medium.tracks)
            .any(|track| track.video != Some(false))
        {
            return Err(Error::Conflict(
                "video tracks or missing recording type prevent an audio-only plan".into(),
            ));
        }
        let tracks = self
            .media
            .iter()
            .flat_map(|medium| {
                medium.tracks.iter().map(|track| PlannedTrack {
                    artist: credit_text(&track.credits),
                    title: track.title.clone(),
                    disc: Some(medium.position),
                    disc_track: Some(track.position),
                })
            })
            .collect();
        Ok(EditionSelection {
            release_id: self.summary.id.clone(),
            album_artist: self.summary.artist.clone(),
            album: self.summary.title.clone(),
            bucket,
            tracks,
        })
    }
}

pub(crate) fn credit_text(credits: &[CatalogCredit]) -> String {
    credits
        .iter()
        .map(|credit| format!("{}{}", credit.name, credit.joinphrase))
        .collect()
}
