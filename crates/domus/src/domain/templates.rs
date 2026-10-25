use super::LibraryPath;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Mp3,
    Flac,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
        }
    }
}

pub struct TrackMetadata<'a> {
    pub album_artist: &'a str,
    pub track_artist: &'a str,
    pub album: &'a str,
    pub title: &'a str,
    pub track: u32,
    pub track_total: u32,
    pub disc: Option<u16>,
    pub disc_track: Option<u16>,
    pub bucket: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct OutputPreview {
    pub relative_folder: String,
    pub filename: String,
    pub changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct OutputTemplate {
    pub folder: String,
    pub filename: String,
}

impl Default for OutputTemplate {
    fn default() -> Self {
        Self {
            folder: "<LETTER>\\<ALBUM ARTIST>\\<ALBUM>".into(),
            filename: "<Track Artist> - <Album> - <Track:00> - <Title>.<ext>".into(),
        }
    }
}

impl OutputTemplate {
    pub fn preview(&self, track: &TrackMetadata<'_>, format: AudioFormat) -> Result<OutputPreview> {
        if track.track == 0 || track.track > track.track_total {
            return Err(Error::InvalidInput(
                "continuous track number is outside the release".into(),
            ));
        }
        if track.disc == Some(0) || track.disc_track == Some(0) {
            return Err(Error::InvalidInput(
                "original disc and track numbers must be positive when present".into(),
            ));
        }
        for value in [
            track.album_artist,
            track.track_artist,
            track.album,
            track.title,
        ] {
            if value.trim().is_empty() || value.len() > 4096 {
                return Err(Error::InvalidInput(
                    "required metadata is empty or too long".into(),
                ));
            }
        }
        if self.folder.len() > 4096 || self.filename.len() > 4096 {
            return Err(Error::InvalidInput("template exceeds 4096 bytes".into()));
        }
        let mut changes = Vec::new();
        let mut folders = Vec::new();
        for component in self.folder.split(['/', '\\']) {
            if matches!(component, "" | "." | "..") || folders.len() >= 32 {
                return Err(Error::InvalidInput(
                    "folder template must contain relative path components".into(),
                ));
            }
            let rendered = render(component, track, format)?;
            folders.push(portable_name(&rendered.to_uppercase(), &mut changes)?);
        }
        if self.filename.contains(['/', '\\']) {
            return Err(Error::InvalidInput(
                "filename template must not contain a directory separator".into(),
            ));
        }
        let filename = portable_name(&render(&self.filename, track, format)?, &mut changes)?;
        if !filename.ends_with(&format!(".{}", format.extension())) {
            return Err(Error::InvalidInput(
                "filename extension does not match the selected format".into(),
            ));
        }
        let relative_folder = folders.join("/");
        LibraryPath::parse(&relative_folder)?;
        Ok(OutputPreview {
            relative_folder,
            filename,
            changes,
        })
    }
}

fn render(template: &str, track: &TrackMetadata<'_>, format: AudioFormat) -> Result<String> {
    let mut output = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('<') {
        let literal = &remaining[..start];
        if literal.contains('>') {
            return Err(Error::InvalidInput("unmatched template delimiter".into()));
        }
        output.push_str(literal);
        let end = remaining[start..]
            .find('>')
            .ok_or_else(|| Error::InvalidInput("unclosed template token".into()))?
            + start;
        let token = &remaining[start + 1..end];
        match token {
            "LETTER" => output.push_str(&letter_bucket(track)?),
            "ALBUM ARTIST" | "Album Artist" => output.push_str(track.album_artist),
            "Track Artist" => output.push_str(track.track_artist),
            "ALBUM" | "Album" => output.push_str(track.album),
            "Title" => output.push_str(track.title),
            "Track" => output.push_str(&track.track.to_string()),
            "Total" => output.push_str(&track.track_total.to_string()),
            "Disc" => output.push_str(
                &track
                    .disc
                    .ok_or_else(|| {
                        Error::InvalidInput("original disc number is unavailable".into())
                    })?
                    .to_string(),
            ),
            "Disc Track" => output.push_str(
                &track
                    .disc_track
                    .ok_or_else(|| {
                        Error::InvalidInput("original disc track is unavailable".into())
                    })?
                    .to_string(),
            ),
            "ext" => output.push_str(format.extension()),
            value if value.starts_with("Track:") => {
                let padding = &value[6..];
                if padding.is_empty() || padding.len() > 6 || !padding.bytes().all(|c| c == b'0') {
                    return Err(Error::InvalidInput(
                        "track padding must contain one through six zeros".into(),
                    ));
                }
                output.push_str(&format!("{:0width$}", track.track, width = padding.len()));
            }
            _ => {
                return Err(Error::InvalidInput(format!(
                    "unknown template token: {token}"
                )));
            }
        }
        remaining = &remaining[end + 1..];
    }
    if remaining.contains('>') {
        return Err(Error::InvalidInput("unmatched template delimiter".into()));
    }
    output.push_str(remaining);
    Ok(output)
}

fn letter_bucket(track: &TrackMetadata<'_>) -> Result<String> {
    if let Some(bucket) = track.bucket {
        if bucket.is_empty() || bucket.contains(['/', '\\']) {
            return Err(Error::InvalidInput(
                "configured bucket must be one nonempty component".into(),
            ));
        }
        return Ok(bucket.to_owned());
    }
    let artist = track.album_artist.trim();
    let artist = if artist
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("The "))
    {
        &artist[4..]
    } else {
        artist
    };
    let initial = artist
        .chars()
        .next()
        .ok_or_else(|| Error::InvalidInput("album artist has no sorting initial".into()))?;
    if initial.is_ascii_alphabetic() {
        Ok(initial.to_ascii_uppercase().to_string())
    } else if initial.is_ascii_digit() {
        Ok("#".into())
    } else {
        Err(Error::InvalidInput(
            "a letter bucket decision is required for this album artist".into(),
        ))
    }
}

fn portable_name(value: &str, changes: &mut Vec<String>) -> Result<String> {
    let mut name: String = value
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    name = name.trim_end_matches([' ', '.']).to_owned();
    if name.is_empty() {
        name.push('_');
    }
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    if reserved {
        name.insert(0, '_');
    }
    if name.encode_utf16().count() > 120 {
        return Err(Error::InvalidInput(
            "output component exceeds the portable 120 UTF-16-unit limit".into(),
        ));
    }
    if name != value {
        changes.push(format!("{value} -> {name}"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> TrackMetadata<'static> {
        TrackMetadata {
            album_artist: "The Cure",
            track_artist: "The Cure",
            album: "Disintegration",
            title: "Plainsong",
            track: 1,
            track_total: 12,
            disc: Some(1),
            disc_track: Some(1),
            bucket: None,
        }
    }

    #[test]
    fn default_preview_matches_approved_contract() -> Result<()> {
        let preview = OutputTemplate::default().preview(&track(), AudioFormat::Mp3)?;
        assert_eq!(preview.relative_folder, "C/THE CURE/DISINTEGRATION");
        assert_eq!(
            preview.filename,
            "The Cure - Disintegration - 01 - Plainsong.mp3"
        );
        assert!(preview.changes.is_empty());
        Ok(())
    }

    #[test]
    fn tag_text_is_not_changed_by_filename_sanitizing() -> Result<()> {
        let mut track = track();
        track.album = "CON";
        track.title = "A/B: C?";
        let preview = OutputTemplate::default().preview(&track, AudioFormat::Flac)?;
        assert!(preview.relative_folder.ends_with("/_CON"));
        assert!(preview.filename.ends_with("A_B_ C_.flac"));
        assert_eq!(track.title, "A/B: C?");
        assert_eq!(preview.changes.len(), 2);
        Ok(())
    }

    #[test]
    fn unknown_sorting_requires_a_decision() -> Result<()> {
        let mut track = track();
        track.album_artist = "東京";
        assert!(
            OutputTemplate::default()
                .preview(&track, AudioFormat::Mp3)
                .is_err()
        );
        track.bucket = Some("東京");
        assert!(
            OutputTemplate::default()
                .preview(&track, AudioFormat::Mp3)
                .is_ok()
        );
        track.album_artist = "2 Artists";
        track.bucket = None;
        assert!(
            OutputTemplate::default()
                .preview(&track, AudioFormat::Mp3)?
                .relative_folder
                .starts_with("#/")
        );
        Ok(())
    }

    #[test]
    fn templates_are_not_code_and_cannot_escape_the_root() {
        let template = OutputTemplate {
            folder: "../<ALBUM>".into(),
            ..OutputTemplate::default()
        };
        assert!(template.preview(&track(), AudioFormat::Mp3).is_err());
        let template = OutputTemplate {
            filename: "<exec:cmd>.mp3".into(),
            ..OutputTemplate::default()
        };
        assert!(template.preview(&track(), AudioFormat::Mp3).is_err());
    }

    #[test]
    fn original_numbering_is_optional_but_must_be_valid() -> Result<()> {
        let mut metadata = track();
        metadata.disc = None;
        metadata.disc_track = None;
        assert!(
            OutputTemplate::default()
                .preview(&metadata, AudioFormat::Mp3)
                .is_ok()
        );
        metadata.disc = Some(0);
        assert!(
            OutputTemplate::default()
                .preview(&metadata, AudioFormat::Mp3)
                .is_err()
        );
        metadata.disc = Some(2);
        metadata.disc_track = Some(3);
        metadata.track = 10;
        let template = OutputTemplate {
            filename: "<Disc>-<Disc Track>-<Track:000>.<ext>".into(),
            ..OutputTemplate::default()
        };
        assert_eq!(
            template.preview(&metadata, AudioFormat::Flac)?.filename,
            "2-3-010.flac"
        );
        Ok(())
    }

    #[test]
    fn the_complete_folder_must_fit_the_shared_relative_path_bound() {
        let album = "\u{00c4}".repeat(70);
        let metadata = TrackMetadata {
            album: &album,
            ..track()
        };
        let template = OutputTemplate {
            folder: vec!["<ALBUM>"; 32].join("\\"),
            ..OutputTemplate::default()
        };
        assert!(template.preview(&metadata, AudioFormat::Mp3).is_err());
    }
}
