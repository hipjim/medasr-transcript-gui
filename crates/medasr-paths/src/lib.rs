//! Per-OS path resolution for app data, model cache, audit log, settings.
//!
//! Uses the `directories` crate for cross-platform XDG / Known Folder
//! compliance. Audit-log paths are intentionally machine-keyed (not
//! user-keyed) so shared OS accounts on radiologist reading-room
//! workstations all write to the same log; see `audit_log_dir`.

#![forbid(unsafe_code)]

use medasr_types::{Error, Result};
use std::path::PathBuf;

const APP_QUALIFIER: &str = "org";
const APP_ORG: &str = "medasr";
const APP_NAME: &str = "medasr";

fn dirs() -> Result<directories::ProjectDirs> {
    directories::ProjectDirs::from(APP_QUALIFIER, APP_ORG, APP_NAME)
        .ok_or_else(|| Error::Other("could not resolve project directories".into()))
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs()?.data_dir().to_path_buf())
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(dirs()?.config_dir().to_path_buf())
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(dirs()?.cache_dir().to_path_buf())
}

pub fn model_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("models"))
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// Audit log directory. Machine-keyed (no user component) so it is safe on
/// shared OS accounts; the `audit.log` file inside is owned by all radiologists
/// using the workstation.
pub fn audit_log_dir() -> Result<PathBuf> {
    // Use the data_dir base — same path for every user on this machine
    // (well, same per OS account on Windows; Linux/macOS keep this in
    // `/Library/Application Support` or `~/.local/share`. Cross-account
    // shared paths are an OS-policy decision documented in PRIVACY.md).
    Ok(data_dir()?.join("audit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve() {
        assert!(data_dir().unwrap().is_absolute());
        assert!(config_dir().unwrap().is_absolute());
        assert!(cache_dir().unwrap().is_absolute());
        assert!(model_cache_dir().unwrap().ends_with("models"));
    }
}
