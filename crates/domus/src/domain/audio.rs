use super::AudioFormat;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const AUDIO_POLICY_VERSION: u8 = 1;

/// File format and encoding quality are separate. VBR has no preset or average
/// bitrate guarantee; validation requires observed audio-frame bitrate variation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum AudioProfile {
    Mp3Cbr128,
    Mp3Cbr160,
    Mp3Cbr192,
    Mp3Cbr224,
    Mp3Cbr256,
    Mp3Cbr320,
    Mp3Vbr,
    FlacLossless,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct AudioPolicy {
    pub version: u8,
    pub accepted: Vec<AudioProfile>,
    pub preferred: AudioProfile,
}

/// Frozen with a plan and carried into its normalization recipe and receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct AudioSelection {
    pub policy_version: u8,
    pub profile: AudioProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct AudioProfileDescriptor {
    pub profile: AudioProfile,
    pub format: AudioFormat,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum AdvertisedQuality {
    Match,
    Mismatch,
    Unknown,
}

impl AudioProfile {
    pub const ALL: [Self; 8] = [
        Self::Mp3Cbr128,
        Self::Mp3Cbr160,
        Self::Mp3Cbr192,
        Self::Mp3Cbr224,
        Self::Mp3Cbr256,
        Self::Mp3Cbr320,
        Self::Mp3Vbr,
        Self::FlacLossless,
    ];

    pub fn legacy(format: AudioFormat) -> Self {
        match format {
            AudioFormat::Mp3 => Self::Mp3Cbr320,
            AudioFormat::Flac => Self::FlacLossless,
        }
    }

    pub fn format(self) -> AudioFormat {
        match self {
            Self::FlacLossless => AudioFormat::Flac,
            _ => AudioFormat::Mp3,
        }
    }

    pub fn bitrate(self) -> Option<u32> {
        match self {
            Self::Mp3Cbr128 => Some(128),
            Self::Mp3Cbr160 => Some(160),
            Self::Mp3Cbr192 => Some(192),
            Self::Mp3Cbr224 => Some(224),
            Self::Mp3Cbr256 => Some(256),
            Self::Mp3Cbr320 => Some(320),
            Self::Mp3Vbr | Self::FlacLossless => None,
        }
    }

    pub fn descriptors() -> Vec<AudioProfileDescriptor> {
        Self::ALL
            .into_iter()
            .map(|profile| AudioProfileDescriptor {
                profile,
                format: profile.format(),
                label: profile.label().into(),
            })
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3Cbr128 => "MP3 · 128 kbps CBR",
            Self::Mp3Cbr160 => "MP3 · 160 kbps CBR",
            Self::Mp3Cbr192 => "MP3 · 192 kbps CBR",
            Self::Mp3Cbr224 => "MP3 · 224 kbps CBR",
            Self::Mp3Cbr256 => "MP3 · 256 kbps CBR",
            Self::Mp3Cbr320 => "MP3 · 320 kbps CBR",
            Self::Mp3Vbr => "MP3 · VBR (no preset or average-bitrate target)",
            Self::FlacLossless => "FLAC · Lossless (no fixed bitrate)",
        }
    }

    pub fn advertised(self, bitrate: Option<u32>, vbr: Option<bool>) -> AdvertisedQuality {
        match self {
            Self::FlacLossless => AdvertisedQuality::Match,
            Self::Mp3Vbr => match vbr {
                Some(true) => AdvertisedQuality::Match,
                Some(false) => AdvertisedQuality::Mismatch,
                None => AdvertisedQuality::Unknown,
            },
            _ => {
                let expected = self.bitrate();
                if vbr == Some(true) || bitrate.is_some_and(|value| Some(value) != expected) {
                    AdvertisedQuality::Mismatch
                } else if bitrate.is_none() || vbr.is_none() {
                    AdvertisedQuality::Unknown
                } else {
                    AdvertisedQuality::Match
                }
            }
        }
    }
}

impl Default for AudioPolicy {
    fn default() -> Self {
        Self {
            version: AUDIO_POLICY_VERSION,
            accepted: vec![AudioProfile::Mp3Cbr320, AudioProfile::FlacLossless],
            preferred: AudioProfile::Mp3Cbr320,
        }
    }
}

impl AudioPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.version != AUDIO_POLICY_VERSION
            || self.accepted.is_empty()
            || self.accepted.len() > AudioProfile::ALL.len()
            || !self.accepted.contains(&self.preferred)
            || self
                .accepted
                .iter()
                .enumerate()
                .any(|(index, profile)| self.accepted[..index].contains(profile))
        {
            return Err(Error::InvalidInput(
                "select distinct supported audio profiles and an accepted preferred profile under audio policy version 1".into(),
            ));
        }
        Ok(())
    }

    pub fn formats(&self) -> Vec<AudioFormat> {
        [AudioFormat::Mp3, AudioFormat::Flac]
            .into_iter()
            .filter(|format| {
                self.accepted
                    .iter()
                    .any(|profile| profile.format() == *format)
            })
            .collect()
    }
}

impl AudioSelection {
    pub fn new(profile: AudioProfile) -> Self {
        Self {
            policy_version: AUDIO_POLICY_VERSION,
            profile,
        }
    }

    pub fn validate(self, format: AudioFormat) -> Result<()> {
        if self.policy_version != AUDIO_POLICY_VERSION || self.profile.format() != format {
            return Err(Error::InvalidInput(
                "audio profile or policy version differs from the selected format".into(),
            ));
        }
        Ok(())
    }
}
