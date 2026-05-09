//! Synthesized-keystroke text injection.
//!
//! - The `KeystrokeBackend` trait abstracts the per-OS keystroke synthesis
//!   primitive. Default impl is `EnigoBackend`; tests use `FakeBackend`.
//! - `Injector` wraps a backend and applies the `FocusTarget`'s
//!   `ChunkingPolicy` so that Citrix targets get small chunks with delays
//!   while native targets get bursts.
//! - **Crucially:** this path NEVER reads or writes the system clipboard.

#![deny(unsafe_op_in_unsafe_fn)]

mod backend;
mod fake;
mod injector;
pub mod secure_input;

pub use backend::{KeystrokeBackend, BackendError};
pub use fake::FakeBackend;
pub use injector::{Injector, InjectError};

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod enigo_backend;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub use enigo_backend::EnigoBackend;
