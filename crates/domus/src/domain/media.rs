use super::identifiers::is_canonical_uuid;
use super::{CatalogCredit, CatalogRelease, credit_text};
use crate::{Error, Result};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrackTags {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album_artist: String,
    pub(crate) album: String,
    pub(crate) track: u32,
    pub(crate) track_total: u32,
    pub(crate) disc: u16,
    pub(crate) disc_total: u16,
    pub(crate) original_track: u16,
    pub(crate) date: Option<String>,
    pub(crate) release_id: String,
    pub(crate) release_group_id: String,
    pub(crate) recording_id: String,
    pub(crate) release_track_id: String,
    pub(crate) artist_ids: Vec<String>,
    pub(crate) album_artist_ids: Vec<String>,
}

fn invalid() -> Error {
    Error::InvalidInput("catalog metadata cannot form complete, bounded audio tags".into())
}

fn validate_text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}

fn artist_ids(credits: &[CatalogCredit]) -> Result<Vec<String>> {
    if credits.is_empty() || credits.len() > 100 {
        return Err(invalid());
    }
    credits
        .iter()
        .map(|credit| {
            validate_text(&credit.name)?;
            if credit.joinphrase.len() > 4096
                || credit.joinphrase.chars().any(char::is_control)
                || !is_canonical_uuid(&credit.artist_id)
            {
                return Err(invalid());
            }
            Ok(credit.artist_id.clone())
        })
        .collect()
}

pub(crate) fn valid_release_date(date: &str) -> bool {
    if !matches!(date.len(), 4 | 7 | 10)
        || !date.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return false;
    }
    let Ok(parts) = date
        .split('-')
        .map(str::parse::<u16>)
        .collect::<std::result::Result<Vec<_>, _>>()
    else {
        return false;
    };
    if parts[0] == 0 || parts.get(1).is_some_and(|month| !(1..=12).contains(month)) {
        return false;
    }
    if let Some(day) = parts.get(2) {
        let year = parts[0];
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let maximum = match parts[1] {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if *day == 0 || *day > maximum {
            return false;
        }
    }
    true
}

impl CatalogRelease {
    /// Use the exact release's ordered mapping. Never copy tags from the peer.
    /// The caller must obtain this release from the approved frozen plan.
    pub fn track_tags(&self, track_number: u32) -> Result<TrackTags> {
        if track_number == 0 {
            return Err(invalid());
        }
        self.audio_tags()?
            .into_iter()
            .nth((track_number - 1) as usize)
            .ok_or_else(invalid)
    }

    pub fn audio_tags(&self) -> Result<Vec<TrackTags>> {
        if self.media.is_empty()
            || self.media.len() > 1000
            || !is_canonical_uuid(&self.summary.id)
            || !is_canonical_uuid(&self.release_group_id)
            || self
                .summary
                .date
                .as_deref()
                .is_some_and(|date| !valid_release_date(date))
        {
            return Err(invalid());
        }
        let album_artist_ids = artist_ids(&self.credits)?;
        let album_artist = credit_text(&self.credits);
        if album_artist != self.summary.artist {
            return Err(invalid());
        }
        let total: usize = self.media.iter().map(|medium| medium.tracks.len()).sum();
        if !(1..=1000).contains(&total) {
            return Err(invalid());
        }
        self.selection(None)?;
        let mut number = 0u32;
        let mut tags = Vec::with_capacity(total);
        for (disc_index, medium) in self.media.iter().enumerate() {
            if usize::from(medium.position) != disc_index + 1 || medium.tracks.is_empty() {
                return Err(invalid());
            }
            for (track_index, track) in medium.tracks.iter().enumerate() {
                if usize::from(track.position) != track_index + 1 {
                    return Err(invalid());
                }
                number += 1;
                let artist_ids = artist_ids(&track.credits)?;
                let artist = credit_text(&track.credits);
                for text in [&track.title, &artist, &self.summary.title, &album_artist] {
                    validate_text(text)?;
                }
                if !is_canonical_uuid(&track.track_id) || !is_canonical_uuid(&track.recording_id) {
                    return Err(invalid());
                }
                tags.push(TrackTags {
                    title: track.title.clone(),
                    artist,
                    album_artist: album_artist.clone(),
                    album: self.summary.title.clone(),
                    track: number,
                    track_total: total as u32,
                    disc: medium.position,
                    disc_total: self.media.len() as u16,
                    original_track: track.position,
                    date: self.summary.date.clone(),
                    release_id: self.summary.id.clone(),
                    release_group_id: self.release_group_id.clone(),
                    recording_id: track.recording_id.clone(),
                    release_track_id: track.track_id.clone(),
                    artist_ids,
                    album_artist_ids: album_artist_ids.clone(),
                });
            }
        }
        Ok(tags)
    }
}
