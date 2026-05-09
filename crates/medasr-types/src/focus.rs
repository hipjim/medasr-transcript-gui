use serde::{Deserialize, Serialize};

/// Chunking policy applied when injecting text into a target window.
///
/// Native targets accept large bursts of synthesized keystrokes. Citrix /
/// Horizon / AVD sessions need smaller chunks with inter-chunk delays
/// because the remote-protocol input pipeline drops or reorders fast bursts.
///
/// The policy lives on `FocusTarget` (not on the injector) so that the
/// orchestrator can pick a per-target policy without the keystroke backend
/// needing to know about Citrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkingPolicy {
    /// Default for native macOS / Windows / Linux text targets.
    Native { chunk_chars: u16, delay_ms: u16 },
    /// Citrix Workspace, VMware Horizon, Microsoft AVD, etc.
    Vdi { chunk_chars: u16, delay_ms: u16 },
}

impl ChunkingPolicy {
    pub const fn native_default() -> Self {
        Self::Native { chunk_chars: 256, delay_ms: 0 }
    }

    pub const fn vdi_default() -> Self {
        Self::Vdi { chunk_chars: 64, delay_ms: 5 }
    }

    pub fn chunk_chars(&self) -> usize {
        match self {
            Self::Native { chunk_chars, .. } | Self::Vdi { chunk_chars, .. } => {
                *chunk_chars as usize
            }
        }
    }

    pub fn delay_ms(&self) -> u64 {
        match self {
            Self::Native { delay_ms, .. } | Self::Vdi { delay_ms, .. } => {
                u64::from(*delay_ms)
            }
        }
    }
}

/// Two-factor focus pin captured at hotkey-press, revalidated at
/// hotkey-release before injection.
///
/// `os_window_id` and `foreground_window_id` are the two factors. They MUST
/// agree at press time; if they disagree (a known Citrix Workspace
/// transparent-shell focus-lag bug), recording is refused. `process_id` plus
/// `secondary_identity` defends against window-ID recycling between press
/// and release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusTarget {
    pub os_window_id: u64,
    pub foreground_window_id: u64,
    pub process_id: u32,
    /// Platform-specific stable identity (AXUIElement on macOS,
    /// `WM_CLIENT_LEADER` on X11, `GetWindowThreadProcessId` companion on
    /// Windows). Used to detect window-ID recycling.
    pub secondary_identity: SecondaryIdentity,
    /// Hashed bundle identifier for audit-log use. Never the raw bundle ID.
    pub bundle_id_hash: Option<[u8; 32]>,
    pub chunking_policy: ChunkingPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecondaryIdentity {
    MacOSAxRef(u64),
    X11ClientLeader(u64),
    WindowsThreadId(u32),
}
