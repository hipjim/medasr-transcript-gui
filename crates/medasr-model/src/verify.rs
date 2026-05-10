//! SHA-256 file integrity check against the compiled-in manifest.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::manifest::ManifestFile;

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("sha256 mismatch for {path}: expected {expected}, got {got}")]
    Mismatch {
        path: String,
        expected: String,
        got: String,
    },
}

/// Verify a file on disk against its manifest entry. Returns Ok(()) on
/// match. If the manifest entry's `sha256_hex` is `"any"` the check is
/// skipped (development-time fallback before the manifest is populated).
pub fn verify_against_manifest(file: &Path, entry: &ManifestFile) -> Result<(), VerifyError> {
    if entry.sha256_hex == "any" {
        tracing::warn!(
            "manifest entry for {} has sha256=\"any\" — integrity check skipped",
            entry.path
        );
        return Ok(());
    }
    let mut hasher = Sha256::new();
    let f = File::open(file).map_err(|e| VerifyError::Io {
        path: file.display().to_string(),
        source: e,
    })?;
    let mut reader = BufReader::new(f);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| VerifyError::Io {
            path: file.display().to_string(),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = hex::encode(hasher.finalize());
    if got.eq_ignore_ascii_case(entry.sha256_hex) {
        Ok(())
    } else {
        Err(VerifyError::Mismatch {
            path: file.display().to_string(),
            expected: entry.sha256_hex.into(),
            got,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    // Hand-computed: sha256 of "hello\n" is
    // 5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03
    #[test]
    fn matches_known_hash() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello\n").unwrap();
        f.sync_all().unwrap();
        let entry = ManifestFile {
            path: "hello.txt",
            sha256_hex: "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
            bytes: 6,
        };
        verify_against_manifest(&path, &entry).unwrap();
    }

    #[test]
    fn detects_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello\n").unwrap();
        let entry = ManifestFile {
            path: "hello.txt",
            sha256_hex: "0000000000000000000000000000000000000000000000000000000000000000",
            bytes: 6,
        };
        let err = verify_against_manifest(&path, &entry).unwrap_err();
        assert!(matches!(err, VerifyError::Mismatch { .. }));
    }

    #[test]
    fn skips_check_when_hash_is_any() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("any.txt");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"whatever").unwrap();
        let entry = ManifestFile {
            path: "any.txt",
            sha256_hex: "any",
            bytes: 8,
        };
        verify_against_manifest(&path, &entry).unwrap();
    }
}
