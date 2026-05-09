//! Post-processing of raw MedASR transcripts.
//!
//! v1 pipeline (synchronous):
//!
//! ```text
//! raw transcript
//!   -> CommandsStage  (voice commands -> punctuation/structural)
//!   -> NumbersStage   (spelled numbers -> digits, with units)
//!   -> CapsStage      (capitalization tidy-up)
//!   -> typed text
//! ```
//!
//! All stages are pure transforms. The trait stays synchronous in v1; a
//! future v2 KenLM-rescoring sidecar wraps this synchronous result in
//! `spawn_blocking` at the orchestrator call site (which is where
//! `spawn_blocking` already lives for injection), so v1's surface
//! survives unchanged.

#![forbid(unsafe_code)]

mod caps;
mod commands;
mod numbers;
mod pipeline;

pub use caps::CapsStage;
pub use commands::CommandsStage;
pub use numbers::NumbersStage;
pub use pipeline::{Pipeline, Stage};

/// Convenience constructor for the v1 default pipeline.
#[must_use]
pub fn default_pipeline() -> Pipeline {
    Pipeline::new()
        .with(CommandsStage::default())
        .with(NumbersStage::default())
        .with(CapsStage::default())
}
