//! Global push-to-talk hotkey wrapper.
//!
//! v1 uses `livesplit-hotkey` because it works as a plain background
//! library (no winit/tao event loop required) so the `medasr-cli`
//! Phase 1 vertical-slice binary can run without a windowed parent. When
//! the Tauri host eventually wires `tauri-plugin-global-shortcut`, it
//! will surface the same `HotkeyEvent` enum through this crate's API.

#![forbid(unsafe_code)]

use std::sync::mpsc;

use livesplit_hotkey::{Hook, Hotkey, KeyCode};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("hook init: {0}")]
    Init(String),
    #[error("registration: {0}")]
    Register(String),
}

/// Events delivered to the orchestrator.
///
/// `livesplit-hotkey` reports edges by way of a single callback per
/// hotkey + an `IsKeyPressed` query. For our push-to-talk model we
/// surface synthetic Press / Release events to keep the orchestrator's
/// state machine simple. On platforms where the underlying lib only
/// reports the press edge, the orchestrator can synthesise a Release on
/// next-press toggle. v1 is press-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
}

pub struct HotkeyService {
    _hook: Hook,
    rx: mpsc::Receiver<HotkeyEvent>,
}

impl HotkeyService {
    /// Register the supplied hotkey. The returned service drops the
    /// hotkey on drop.
    pub fn register(hotkey: Hotkey) -> Result<Self, HotkeyError> {
        let hook = Hook::new().map_err(|e| HotkeyError::Init(format!("{e:?}")))?;
        let (tx, rx) = mpsc::channel();
        hook.register(hotkey, move || {
            info!("hotkey pressed");
            let _ = tx.send(HotkeyEvent::Pressed);
        })
        .map_err(|e| HotkeyError::Register(format!("{e:?}")))?;
        Ok(Self { _hook: hook, rx })
    }

    /// Block until the next hotkey event arrives.
    pub fn recv(&self) -> Option<HotkeyEvent> {
        self.rx.recv().ok()
    }

    /// Try to drain any pending events without blocking.
    pub fn try_recv(&self) -> Option<HotkeyEvent> {
        self.rx.try_recv().ok()
    }
}

/// The default v1 hotkey: F12 (no modifiers). Foot pedals typically
/// emit a single keycode on press; F12 maps cleanly to common pedal
/// programming.
pub fn default_hotkey() -> Hotkey {
    Hotkey {
        key_code: KeyCode::F12,
        modifiers: livesplit_hotkey::Modifiers::empty(),
    }
}
