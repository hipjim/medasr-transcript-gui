//! Compile-time-pinned model manifest.
//!
//! The plan's original design called for a Minisign-signed
//! `release-bundle.bin` blob containing manifest + SPKI pins. Per the
//! personal/research distribution posture chosen during review, we collapse
//! that to a Rust `const` baked into the binary: tampering with model bytes
//! after download will fail the SHA-256 check; tampering with the binary
//! itself is the trust anchor we already rely on for the rest of the code.
//!
//! Update this when changing the pinned model revision. The pinned revision
//! is the commit SHA on Hugging Face — `main` is intentionally not used so
//! that an upstream force-push cannot silently substitute weights.

/// Hugging Face repo owning the int8 ONNX export of MedASR-CTC.
pub const HF_REPO: &str = "csukuangfj/sherpa-onnx-medasr-ctc-en-int8-2025-12-25";

/// Revision SHA pinned at build time. **Update via release process when
/// bumping the model.** The placeholder string here means "fetch the
/// `main` branch tip"; replace with a 40-hex commit SHA before tagging
/// any release.
pub const HF_REVISION: &str = "main";

/// One file entry in the manifest. SHA-256 is verified at download
/// completion AND at every app launch (mmap-and-hash, fast on these
/// sizes).
#[derive(Debug, Clone, Copy)]
pub struct ManifestFile {
    pub path: &'static str,
    pub sha256_hex: &'static str,
    pub bytes: u64,
}

/// Minimal file set for v1 inference: the int8 ONNX model + tokens. The
/// 944 MB KenLM bundle is intentionally NOT included (R5 — LM rescoring
/// is a v2 feature).
///
/// Real SHA-256s are populated by the build process (or `update-manifest`
/// helper script) the first time we cut a release. Until then these are
/// "any" — verify_against_manifest will skip the check if `sha256_hex` is
/// the literal `"any"`.
pub const MANIFEST: &[ManifestFile] = &[
    ManifestFile {
        path: "model.int8.onnx",
        sha256_hex: "any",
        bytes: 0,
    },
    ManifestFile {
        path: "tokens.txt",
        sha256_hex: "any",
        bytes: 0,
    },
];

pub fn lookup(path: &str) -> Option<&'static ManifestFile> {
    MANIFEST.iter().find(|f| f.path == path)
}
