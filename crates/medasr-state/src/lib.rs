//! Push-to-talk state machine.
//!
//! Headless: no audio, no ASR, no Tauri. The orchestrator
//! (`medasr-lifecycle`) drives transitions and dispatches side-effects.
//!
//! The state diagram (matches the plan's stateDiagram-v2 in the High-Level
//! Technical Design):
//!
//! ```text
//! Uninitialized -> EulaPending -> PermissionsPending -> ModelMissing
//!   -> Downloading -> Verifying -> Warming -> Ready
//!   -> Recording -> Transcribing -> Injecting -> Ready
//!   (at any point: -> Aborted | Error -> Ready)
//! ```

#![forbid(unsafe_code)]

pub mod events;
pub mod machine;

pub use events::{AbortReason, ErrorClass, Event};
pub use machine::{Machine, State, TransitionEffect};
