//! HAI-DEF EULA acceptance flow.
//!
//! We bundle the EULA text into the binary, hash it with SHA-256, and
//! record acceptance keyed on `(app_version, eula_hash)` so:
//! - downgrading the app re-prompts (different version),
//! - shipping a new EULA version re-prompts (different hash),
//! - day-to-day app launches don't re-prompt.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// HAI-DEF Terms of Use text. The actual canonical text is at
/// <https://developers.google.com/health-ai-developer-foundations/terms>;
/// this binding is updated alongside model-revision bumps.
///
/// In v1 we ship a stub that explicitly tells the user to read the upstream
/// terms — shipping a stale copy of HAI-DEF text in a binary is itself a
/// minor risk (the canonical text can change), so we surface a "I have
/// read the HAI-DEF terms at <url>" acceptance rather than reproducing it
/// in full.
pub const EULA_TEXT: &str = include_str!("eula_text.md");

#[derive(Debug, Error)]
pub enum EulaError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EulaAcceptance {
    pub app_version: String,
    pub eula_hash_hex: String,
    pub accepted_iso8601: String,
}

pub fn current_hash_hex() -> String {
    let mut h = Sha256::new();
    h.update(EULA_TEXT.as_bytes());
    hex::encode(h.finalize())
}

pub fn current_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns true if a previously-recorded acceptance matches the *current*
/// app_version + eula_hash. Anything else (including missing file)
/// indicates the user must be re-prompted.
pub fn is_accepted(record_path: &std::path::Path) -> Result<bool, EulaError> {
    if !record_path.exists() {
        return Ok(false);
    }
    let bytes = std::fs::read(record_path)?;
    let rec: EulaAcceptance = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(_) => return Ok(false),
    };
    Ok(rec.app_version == current_app_version() && rec.eula_hash_hex == current_hash_hex())
}

pub fn record_acceptance(record_path: &std::path::Path) -> Result<EulaAcceptance, EulaError> {
    let rec = EulaAcceptance {
        app_version: current_app_version().into(),
        eula_hash_hex: current_hash_hex(),
        accepted_iso8601: now_iso8601(),
    };
    if let Some(parent) = record_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(record_path, serde_json::to_vec_pretty(&rec)?)?;
    Ok(rec)
}

pub fn standard_record_path() -> PathBuf {
    medasr_paths::config_dir()
        .map(|d| d.join("eula-acceptance.json"))
        .unwrap_or_else(|_| PathBuf::from("eula-acceptance.json"))
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Minimal ISO 8601 without dragging in chrono. Seconds-since-epoch is
    // not useful for forensic compliance but is sufficient for v1's
    // record-of-acceptance purpose.
    format!("@{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_acceptance() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("acceptance.json");
        assert!(!is_accepted(&path).unwrap());
        record_acceptance(&path).unwrap();
        assert!(is_accepted(&path).unwrap());
    }

    #[test]
    fn missing_file_is_not_accepted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(!is_accepted(&path).unwrap());
    }

    #[test]
    fn corrupt_file_is_not_accepted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(!is_accepted(&path).unwrap());
    }

    #[test]
    fn version_mismatch_re_prompts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("acceptance.json");
        let rec = EulaAcceptance {
            app_version: "0.0.0-stale".into(),
            eula_hash_hex: current_hash_hex(),
            accepted_iso8601: "@0".into(),
        };
        std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
        assert!(!is_accepted(&path).unwrap());
    }

    #[test]
    fn eula_hash_change_re_prompts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("acceptance.json");
        let rec = EulaAcceptance {
            app_version: current_app_version().into(),
            eula_hash_hex: "0".repeat(64),
            accepted_iso8601: "@0".into(),
        };
        std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
        assert!(!is_accepted(&path).unwrap());
    }
}
