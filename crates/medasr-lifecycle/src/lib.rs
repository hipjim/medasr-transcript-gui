//! Orchestrator: ties the audio, ASR, post-process, focus, and inject
//! workers together under the state-machine in `medasr-state`.
//!
//! ## Concurrency model
//!
//! Single owning thread (the orchestrator) holds:
//! - the hotkey channel,
//! - the cpal `AudioCapture` (when active),
//! - the ASR worker handle (mpsc producer),
//! - the `Injector<EnigoBackend>` (which is `!Send` on macOS, so confined
//!   to this thread).
//!
//! The state machine in `medasr-state::Machine` runs in-process; effects
//! it emits are dispatched synchronously from this thread. There is one
//! background OS thread (the ASR worker, owned by `AsrWorkerHandle`); we
//! talk to it over its mpsc.
//!
//! ## Why no Tokio
//!
//! v1 doesn't have any I/O concurrency that benefits from async. cpal,
//! ASR, and Enigo are all synchronous-and-blocking. The orchestrator is
//! a plain `std::thread` reading from `mpsc::Receiver<HotkeyEvent>`.

#![forbid(unsafe_code)]

mod orchestrator;

pub use orchestrator::{
    build as orchestrator_build, Orchestrator, OrchestratorError, RunOnce, RunOnceOutcome,
};
