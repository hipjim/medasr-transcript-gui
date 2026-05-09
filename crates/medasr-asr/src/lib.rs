//! Offline batch ASR via the official `sherpa-onnx` Rust crate.
//!
//! Pre-Unit-1 spike confirmed `sherpa-onnx 1.13.x` (Apache-2.0, upstream
//! `k2-fsa/sherpa-onnx`) exposes `OfflineMedAsrCtcModelConfig`,
//! `OfflineRecognizer`, `OfflineRecognizerConfig`, and
//! `OfflineRecognizerResult` for MedASR-CTC.
//!
//! ## Honest residency note
//!
//! sherpa-onnx's `accept_waveform` takes `&[f32]`. Our pipeline produces
//! `SecureBuffer<i16>` from `medasr-audio`. We convert into a transient
//! `SecureBuffer<f32>` so the f32 copy is also mlock'd / zero-on-drop —
//! but **inside** sherpa-onnx (and the underlying ONNX Runtime) the
//! samples and intermediate tensors live in ordinary heap. R11 is honest
//! only about the audio buffer + final transcript; everything in between
//! is documented as a residual gap in `docs/PRIVACY.md`.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod recognizer;
pub mod worker;

pub use recognizer::{Asr, AsrError, ModelPaths};
pub use worker::{spawn_worker, AsrCommand, AsrResponse, AsrWorkerHandle};
