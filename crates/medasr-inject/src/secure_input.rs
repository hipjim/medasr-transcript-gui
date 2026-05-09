//! macOS Secure Event Input probe.
//!
//! When a sandboxed app such as Touch ID or 1Password's password fields,
//! sudo's terminal prompt, or a password input that calls
//! `EnableSecureEventInput()` is focused, macOS suppresses synthesized
//! keystrokes silently. We detect this so:
//!
//!   1. Before injecting, we abort with `SecureInputBlocked` rather than
//!      typing a partial transcript into the wrong place.
//!   2. Per the plan, we probe again BEFORE EVERY CHUNK so a Secure Input
//!      window that engages mid-injection results in `SecureInputBlocked`-
//!      mid-stream rather than a silent loss of the rest of the transcript.
//!
//! On non-macOS this is a no-op (always returns false).

#[cfg(target_os = "macos")]
mod imp {
    extern "C" {
        // From <Carbon/HIToolbox/Events.h>
        fn IsSecureEventInputEnabled() -> u8;
    }
    pub fn is_secure_input_enabled() -> bool {
        // SAFETY: zero-arg C call returning a Boolean (u8).
        unsafe { IsSecureEventInputEnabled() != 0 }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn is_secure_input_enabled() -> bool { false }
}

/// Probe whether macOS Secure Event Input is currently engaged.
///
/// In non-test builds this calls the `IsSecureEventInputEnabled` Carbon API.
/// In test builds (and any build where `MEDASR_FORCE_INSECURE_INPUT=1` is
/// set), it returns false unconditionally so tests using `FakeBackend`
/// don't fail on dev hosts that happen to have 1Password unlocked.
pub fn is_secure_input_enabled() -> bool {
    if cfg!(test) {
        return false;
    }
    if std::env::var_os("MEDASR_FORCE_INSECURE_INPUT").is_some() {
        return false;
    }
    imp::is_secure_input_enabled()
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_returns_a_bool() {
        // Just exercise the symbol; we can't assert a specific value
        // because Secure Input may or may not be active on the test
        // host (it commonly is on macOS when 1Password is unlocked).
        let _ = super::is_secure_input_enabled();
    }
}
