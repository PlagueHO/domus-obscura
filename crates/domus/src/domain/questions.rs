use crate::{Error, Result};
use std::collections::HashSet;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    ts_rs::TS,
    thiserror::Error,
)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkIssue {
    #[error("the album needs a permitted cover")]
    Missing,
    #[error("confirm permitted use before preparing this cover")]
    Permission,
    #[error("approve the exact JPEG preview before continuing")]
    PreviewApproval,
    #[error("transparent cover needs an explicit background color")]
    Background,
    #[error("animated cover needs a supported static replacement")]
    Animated,
    #[error("profiled cover needs a supported color-management replacement")]
    ColorProfile,
    #[error("cover precision, color space or format needs a supported-image replacement")]
    Unsupported,
}

impl ArtworkIssue {
    pub fn prompt(self) -> String {
        format!(
            "{self}. Prepare a permitted supported cover, check its JPEG preview, and approve its exact hash. Then choose Use approved cover."
        )
    }
}

pub struct Question {
    prompt: String,
    choices: Vec<String>,
}

impl Question {
    pub fn new(prompt: String, choices: Vec<String>) -> Result<Self> {
        if prompt.trim().is_empty()
            || prompt.len() > 2048
            || !(1..=8).contains(&choices.len())
            || choices
                .iter()
                .any(|choice| choice.trim().is_empty() || choice.len() > 128)
            || choices.iter().collect::<HashSet<_>>().len() != choices.len()
        {
            return Err(Error::InvalidInput(
                "invalid question prompt or choices".into(),
            ));
        }
        Ok(Self { prompt, choices })
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn choices(&self) -> &[String] {
        &self.choices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_distinct_bounded_choices() {
        assert!(
            Question::new("Choose a format".into(), vec!["FLAC".into(), "FLAC".into()]).is_err()
        );
        assert!(Question::new("".into(), vec!["FLAC".into()]).is_err());
        assert!(Question::new("Choose a format".into(), vec![]).is_err());
        assert!(Question::new("Choose a format".into(), vec!["FLAC".into()]).is_ok());
    }
}
