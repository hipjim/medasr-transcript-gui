//! Offline batch ASR via sherpa-onnx. Implementation lands in Unit 3.
//!
//! Spike (pre-Unit-1) confirmed:
//! - Official `sherpa-onnx = 1.13.1` crate (Apache-2.0, upstream
//!   `k2-fsa/sherpa-onnx`) exposes `MedAsrCtcModelConfig`,
//!   `OfflineRecognizer`, `OfflineRecognizerConfig`, `OfflineRecognizerResult`.
//! - Default `static` feature builds the C++ library statically (good for
//!   distribution; first build is slow).
//! - Real model load + transcribe smoke deferred to Unit 3.
#![forbid(unsafe_code)]
