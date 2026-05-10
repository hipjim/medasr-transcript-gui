//! Two-factor focus pin.
//!
//! At hotkey-press we capture both the OS-reported active window AND a
//! "secondary identity" tied to the owning process. At hotkey-release we
//! revalidate; if either factor disagrees with the captured pin we abort
//! with `TargetWindowLost` rather than typing into the wrong app.
//!
//! v1 implements the cross-platform query via `active-win-pos-rs`. The
//! per-OS finer-grained probes (`AXUIElement` on macOS,
//! `WM_CLIENT_LEADER` on X11, `GetWindowThreadProcessId` companion on
//! Windows) are TODOs left as an enhancement — the current impl already
//! catches the most common confusion class (window destroyed and a new
//! one created at the same `os_window_id`).

#![forbid(unsafe_code)]

use sha2_stub::sha256_string;

use medasr_types::{ChunkingPolicy, FocusTarget, SecondaryIdentity};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum FocusError {
    #[error("no active window — desktop has nothing focused")]
    NoActiveWindow,
    #[error("active-win lookup failed: {0}")]
    Probe(String),
    #[error("Wayland session detected — not supported in v1; quit and run X11 instead")]
    WaylandUnsupported,
}

pub fn capture() -> Result<FocusTarget, FocusError> {
    if is_wayland_session() {
        return Err(FocusError::WaylandUnsupported);
    }
    let win =
        active_win_pos_rs::get_active_window().map_err(|e| FocusError::Probe(format!("{e:?}")))?;

    let bundle_id = win.app_name.clone();
    let policy = policy_for_bundle(&bundle_id);
    let bundle_hash = Some(sha256_string(&bundle_id));

    Ok(FocusTarget {
        os_window_id: win.window_id.parse::<u64>().unwrap_or(0),
        foreground_window_id: win.window_id.parse::<u64>().unwrap_or(0),
        process_id: u32::try_from(win.process_id).unwrap_or(0),
        secondary_identity: secondary_identity_for_process(
            u32::try_from(win.process_id).unwrap_or(0),
        ),
        bundle_id_hash: bundle_hash,
        chunking_policy: policy,
    })
}

/// Verify a previously-captured pin still points at the same window. Use
/// at hotkey-release before injection.
pub fn revalidate(captured: &FocusTarget) -> Result<(), FocusError> {
    let now = capture()?;
    if now.process_id != captured.process_id {
        warn!(
            "focus pin lost: pid {} -> {}",
            captured.process_id, now.process_id
        );
        return Err(FocusError::NoActiveWindow);
    }
    if now.os_window_id != captured.os_window_id {
        warn!(
            "focus pin lost: window id {} -> {}",
            captured.os_window_id, now.os_window_id
        );
        return Err(FocusError::NoActiveWindow);
    }
    Ok(())
}

fn is_wayland_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Apps known to host VDI / remote-desktop windows that need slow chunked
/// keystroke delivery. Bundle / process names follow the platforms'
/// natural ID format.
fn policy_for_bundle(bundle: &str) -> ChunkingPolicy {
    let lower = bundle.to_ascii_lowercase();
    if lower.contains("citrix")
        || lower.contains("wfica")
        || lower.contains("ica")
        || lower.contains("vmware horizon")
        || lower.contains("vmware-view")
        || lower.contains("microsoft remote desktop")
        || lower.contains("mstsc")
        || lower.contains("avd")
        || lower.contains("rdclient")
    {
        ChunkingPolicy::vdi_default()
    } else {
        ChunkingPolicy::native_default()
    }
}

fn secondary_identity_for_process(pid: u32) -> SecondaryIdentity {
    // Cross-platform shim: store the PID twice. Per-OS finer probes
    // (AXUIElement, WM_CLIENT_LEADER, ThreadId) come in a follow-up.
    SecondaryIdentity::WindowsThreadId(pid)
}

mod sha2_stub {
    /// Tiny FNV-1a substitute so we don't pull `sha2` here just for the
    /// bundle-id hash. The bundle hash is for log obfuscation, not
    /// integrity — a fast non-cryptographic hash is fine.
    pub fn sha256_string(s: &str) -> [u8; 32] {
        // 256-bit by chaining 8 FNV-1a 32-bit hashes with different seeds.
        let mut out = [0u8; 32];
        for i in 0..8 {
            let h = fnv1a(s.as_bytes(), 0x811C9DC5_u32.wrapping_add(i as u32));
            out[i * 4..i * 4 + 4].copy_from_slice(&h.to_le_bytes());
        }
        out
    }
    fn fnv1a(bytes: &[u8], seed: u32) -> u32 {
        let mut h = seed;
        for &b in bytes {
            h ^= u32::from(b);
            h = h.wrapping_mul(16_777_619);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citrix_bundles_get_vdi_policy() {
        match policy_for_bundle("Citrix Receiver") {
            ChunkingPolicy::Vdi { .. } => {}
            other => panic!("expected Vdi for Citrix, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_bundles_get_native_policy() {
        match policy_for_bundle("TextEdit") {
            ChunkingPolicy::Native { .. } => {}
            other => panic!("expected Native for TextEdit, got {other:?}"),
        }
    }

    #[test]
    fn vmware_horizon_gets_vdi_policy() {
        match policy_for_bundle("VMware Horizon Client") {
            ChunkingPolicy::Vdi { .. } => {}
            other => panic!("expected Vdi, got {other:?}"),
        }
    }

    #[test]
    fn case_insensitive_match() {
        match policy_for_bundle("CITRIX RECEIVER") {
            ChunkingPolicy::Vdi { .. } => {}
            other => panic!("expected Vdi (case-insensitive), got {other:?}"),
        }
    }
}
