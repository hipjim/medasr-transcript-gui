//! Push-to-talk state machine.
//!
//! This crate is headless: it never depends on platform crates (no audio,
//! no inject, no focus, no Tauri). The orchestrator in `medasr-lifecycle`
//! drives transitions and dispatches side-effects.
//!
//! Full impl lands in Unit 5; this stub fixes the state names so other
//! crates can reference them.

#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Uninitialized,
    EulaPending,
    PermissionsPending,
    ModelMissing,
    Downloading,
    Verifying,
    Warming,
    Ready,
    Recording,
    Transcribing,
    Injecting,
    Aborted,
    Error,
}
