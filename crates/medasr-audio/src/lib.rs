//! Cross-platform audio capture pipeline.
//!
//! Pipeline shape (driven by `medasr-lifecycle`):
//!
//! ```text
//! cpal callback ──► rtrb ring ──► consumer drains ──► resample to 16 kHz i16
//!     OS-priority      SPSC          orchestrator       (rubato)
//!     thread           lock-free                        ──► VAD gate
//!                                                       ──► SecureBuffer<i16>
//!                                                            handed to ASR
//! ```
//!
//! All PHI-bearing audio buffers flow through `SecureBuffer<i16>` from
//! `medasr-secure-buffer` so they are mlock'd and zeroed on drop. The plain
//! `Vec<i16>` types in this module are scratch / FFI-boundary types only.

#![forbid(unsafe_code)]

pub mod buffer;
pub mod capture;
pub mod preprocess;
pub mod resample;
pub mod vad;

pub use buffer::{RecordingBuilder, RecordingOutcome};
pub use capture::{AudioCapture, CaptureConfig, CaptureError};
pub use preprocess::{high_pass_filter, rms_normalize, trim_silence};
pub use resample::{resample_to_16k_mono, ResampleError};
pub use vad::{EnergyVad, VadDecision, VadParams};
