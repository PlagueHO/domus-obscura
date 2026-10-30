use serde::{Deserialize, Serialize};

/// A separate user choice is required when the planned album folder already
/// contains files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementAction {
    Skip,
    Copy,
    Replace,
}
