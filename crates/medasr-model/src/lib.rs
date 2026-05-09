//! Model fetch + verify + HAI-DEF EULA gating.
//!
//! Per the personal/research distribution posture chosen at scaffold time:
//!
//! - System trust store via rustls + native roots; no SPKI pinning.
//! - Compile-time SHA-256 manifest is the integrity guarantee.
//! - No Minisign-signed `release-bundle.bin`.
//! - No OS-level egress filter (Network Extension / WFP / nftables).
//!
//! The seam to add those back later is the manifest module: extending
//! `ManifestFile` with an SPKI list and pulling rustls-tls (no native
//! roots) is a localized change.

#![forbid(unsafe_code)]

pub mod eula;
pub mod fetch;
pub mod manifest;
pub mod verify;

pub use eula::{is_accepted, record_acceptance, standard_record_path, EulaAcceptance, EulaError};
pub use fetch::{default_client, fetch_file, FetchError, Progress};
pub use manifest::{ManifestFile, HF_REPO, HF_REVISION, MANIFEST};
pub use verify::{verify_against_manifest, VerifyError};
