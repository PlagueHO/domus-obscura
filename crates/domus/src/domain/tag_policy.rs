use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TagMode {
    Automatic,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TagRule {
    Update { value: String, mode: TagMode },
    Retain {},
    Remove { mode: TagMode },
}

impl Default for TagRule {
    fn default() -> Self {
        Self::Remove {
            mode: TagMode::Automatic,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct OptionalTags {
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub copyright: Option<String>,
    pub comment_language: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct TagPolicy {
    pub genre: TagRule,
    pub composer: TagRule,
    pub comment: TagRule,
    pub copyright: TagRule,
}

pub(crate) fn tag_text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "tag text must be non-empty, contain no control characters, and fit in 4096 UTF-8 bytes"
                .into(),
        ));
    }
    Ok(())
}

impl TagRule {
    pub fn needs_review(&self) -> bool {
        !matches!(
            self,
            Self::Update {
                mode: TagMode::Automatic,
                ..
            } | Self::Remove {
                mode: TagMode::Automatic
            }
        )
    }

    fn resolve(&self, source: &Option<String>) -> Result<Option<String>> {
        let value = match self {
            Self::Update { value, .. } => Some(value.clone()),
            Self::Retain {} => source.clone(),
            Self::Remove { .. } => None,
        };
        if let Some(value) = &value {
            tag_text(value)?;
        }
        Ok(value)
    }
}

impl TagPolicy {
    pub fn resolve_for_format(
        &self,
        source: &OptionalTags,
        format: super::AudioFormat,
    ) -> Result<OptionalTags> {
        let mut result = self.resolve(source)?;
        if format == super::AudioFormat::Flac {
            result.comment_language = None;
        }
        Ok(result)
    }

    pub fn needs_review(&self) -> bool {
        [&self.genre, &self.composer, &self.comment, &self.copyright]
            .into_iter()
            .any(TagRule::needs_review)
    }

    pub fn validate(&self) -> Result<()> {
        self.resolve(&OptionalTags::default()).map(|_| ())
    }

    /// Resolve only validated source facts. Transport parsers must reject
    /// duplicates before constructing the single-value source representation.
    pub fn resolve(&self, source: &OptionalTags) -> Result<OptionalTags> {
        let result = OptionalTags {
            genre: self.genre.resolve(&source.genre)?,
            composer: self.composer.resolve(&source.composer)?,
            comment: self.comment.resolve(&source.comment)?,
            copyright: self.copyright.resolve(&source.copyright)?,
            comment_language: match &self.comment {
                TagRule::Retain {} if source.comment.is_some() => source.comment_language.clone(),
                TagRule::Update { .. } => Some("eng".into()),
                _ => None,
            },
        };
        if result.copyright.as_ref().is_some_and(|text| {
            text.len() < 6
                || !text.as_bytes()[..4].iter().all(u8::is_ascii_digit)
                || &text.as_bytes()[..4] == b"0000"
                || text.as_bytes()[4] != b' '
        }) {
            return Err(Error::InvalidInput(
                "Copyright must start with a four-digit year and a space".into(),
            ));
        }
        if result.comment_language.as_ref().is_some_and(|language| {
            language.len() != 3 || !language.bytes().all(|byte| byte.is_ascii_lowercase())
        }) {
            return Err(Error::InvalidInput(
                "comment language must contain three lowercase ASCII letters".into(),
            ));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_removal_is_default_and_retention_always_needs_review() -> Result<()> {
        let mut policy = TagPolicy::default();
        let source = OptionalTags {
            genre: Some("  Rock / Pop  ".into()),
            comment: Some("unwanted".into()),
            ..Default::default()
        };
        assert!(!policy.needs_review());
        assert_eq!(policy.resolve(&source)?, OptionalTags::default());
        policy.genre = TagRule::Retain {};
        assert!(policy.needs_review());
        assert_eq!(policy.resolve(&source)?.genre, source.genre);
        assert!(policy.resolve(&OptionalTags::default())?.genre.is_none());
        Ok(())
    }

    #[test]
    fn updates_and_retention_reject_invalid_values_without_changing_text() -> Result<()> {
        for value in ["".into(), " ".into(), "bad\ntext".into(), "x".repeat(4097)] {
            let policy = TagPolicy {
                composer: TagRule::Update {
                    value,
                    mode: TagMode::Automatic,
                },
                ..Default::default()
            };
            assert!(policy.validate().is_err());
        }
        for invalid in [
            r#"{"action":"retain","mode":"automatic"}"#,
            r#"{"action":"retain","value":"forbidden"}"#,
            r#"{"action":"remove","mode":"review","value":"forbidden"}"#,
            r#"{"action":"update","value":"missing mode"}"#,
        ] {
            assert!(serde_json::from_str::<TagRule>(invalid).is_err());
        }
        assert!(tag_text(&"é".repeat(2048)).is_ok());
        assert!(tag_text(&"é".repeat(2049)).is_err());
        let policy = TagPolicy {
            composer: TagRule::Retain {},
            ..Default::default()
        };
        let source = OptionalTags {
            composer: Some("  Björk  ".into()),
            ..Default::default()
        };
        assert_eq!(policy.resolve(&source)?.composer, source.composer);
        Ok(())
    }
}
