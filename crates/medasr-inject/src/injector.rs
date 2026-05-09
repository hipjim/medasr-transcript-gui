//! `Injector` — applies a `FocusTarget`'s chunking policy and dispatches
//! to a `KeystrokeBackend`.

use std::thread::sleep;
use std::time::Duration;

use thiserror::Error;

use medasr_types::{ChunkingPolicy, FocusTarget};

use crate::backend::{BackendError, KeystrokeBackend};

#[derive(Debug, Error)]
pub enum InjectError {
    #[error("backend: {0}")]
    Backend(#[from] BackendError),
    #[error("macOS Secure Input is engaged — keystrokes would be silently dropped")]
    SecureInputBlocked,
    #[error("macOS Secure Input engaged mid-stream after {chunks_typed} chunks; the partially-typed text is in the focused window")]
    SecureInputBlockedMidStream { chunks_typed: usize },
}

pub struct Injector<B: KeystrokeBackend> {
    backend: B,
}

impl<B: KeystrokeBackend> Injector<B> {
    pub fn new(backend: B) -> Self { Self { backend } }

    pub fn into_backend(self) -> B { self.backend }
    pub fn backend_mut(&mut self) -> &mut B { &mut self.backend }

    /// Inject `text` according to `target.chunking_policy`. Native targets
    /// receive a single (or few) burst; VDI targets receive small chunks
    /// with inter-chunk delays. macOS Secure Input is probed BEFORE EVERY
    /// chunk so a password field engaging mid-stream surfaces an explicit
    /// error rather than silently dropping the rest of the transcript.
    pub fn inject(&mut self, text: &str, target: &FocusTarget) -> Result<(), InjectError> {
        if crate::secure_input::is_secure_input_enabled() {
            return Err(InjectError::SecureInputBlocked);
        }
        let policy = target.chunking_policy;
        let chunk_chars = policy.chunk_chars().max(1);
        let delay = Duration::from_millis(policy.delay_ms());

        let mut start = 0;
        let bytes = text.as_bytes();
        let mut chunks_typed = 0usize;
        while start < bytes.len() {
            // Per-chunk Secure Input probe.
            if chunks_typed > 0 && crate::secure_input::is_secure_input_enabled() {
                return Err(InjectError::SecureInputBlockedMidStream { chunks_typed });
            }
            let mut chars = 0;
            let mut end = start;
            while end < bytes.len() && chars < chunk_chars {
                end += utf8_codepoint_len(bytes[end]);
                chars += 1;
            }
            let chunk = std::str::from_utf8(&bytes[start..end])
                .expect("chunk on char boundary");
            self.backend.type_unicode_string(chunk)?;
            self.backend.flush()?;
            chunks_typed += 1;
            start = end;
            if start < bytes.len() && !delay.is_zero() {
                sleep(delay);
            }
        }
        Ok(())
    }

    pub fn inject_with_policy(&mut self, text: &str, _policy: ChunkingPolicy) -> Result<(), InjectError> {
        // Convenience wrapper for tests / CLI; constructs a synthetic
        // FocusTarget with just the policy field that matters here.
        let dummy = FocusTarget {
            os_window_id: 0,
            foreground_window_id: 0,
            process_id: 0,
            secondary_identity: medasr_types::SecondaryIdentity::WindowsThreadId(0),
            bundle_id_hash: None,
            chunking_policy: _policy,
        };
        self.inject(text, &dummy)
    }
}

fn utf8_codepoint_len(first_byte: u8) -> usize {
    match first_byte {
        b if b < 0x80 => 1,
        b if b < 0xC0 => 1, // invalid as a leading byte; advance 1 to make progress
        b if b < 0xE0 => 2,
        b if b < 0xF0 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeBackend;

    #[test]
    fn native_policy_sends_single_chunk_under_threshold() {
        let backend = FakeBackend::new();
        let probe = backend.clone();
        let mut inj = Injector::new(backend);
        let target = mk_target(ChunkingPolicy::native_default()); // 256/0
        inj.inject("Hello world", &target).unwrap();
        assert_eq!(probe.calls(), vec!["Hello world".to_string()]);
    }

    #[test]
    fn native_policy_chunks_at_256() {
        let backend = FakeBackend::new();
        let probe = backend.clone();
        let mut inj = Injector::new(backend);
        let target = mk_target(ChunkingPolicy::native_default());
        let text: String = "a".repeat(500);
        inj.inject(&text, &target).unwrap();
        assert_eq!(probe.call_count(), 2);
        assert_eq!(probe.typed(), text);
    }

    #[test]
    fn vdi_policy_chunks_at_64_with_delay() {
        let backend = FakeBackend::new();
        let probe = backend.clone();
        let mut inj = Injector::new(backend);
        let target = mk_target(ChunkingPolicy::vdi_default()); // 64/5
        let text: String = "a".repeat(500);
        inj.inject(&text, &target).unwrap();
        // ceil(500 / 64) = 8 chunks.
        assert_eq!(probe.call_count(), 8);
        assert_eq!(probe.typed(), text);
    }

    #[test]
    fn unicode_chunks_on_char_boundaries() {
        let backend = FakeBackend::new();
        let probe = backend.clone();
        let mut inj = Injector::new(backend);
        // chunk_chars = 2; verifies multi-byte chars don't split.
        let target = mk_target(ChunkingPolicy::Vdi { chunk_chars: 2, delay_ms: 0 });
        // "× °" — 4-byte multi-byte chars.
        let text = "×°×°×°";
        inj.inject(text, &target).unwrap();
        let calls = probe.calls();
        // 6 chars / 2 per chunk = 3 chunks.
        assert_eq!(calls.len(), 3);
        for c in &calls {
            // Each chunk must itself be valid UTF-8.
            assert!(std::str::from_utf8(c.as_bytes()).is_ok());
        }
        assert_eq!(probe.typed(), text);
    }

    #[test]
    fn empty_string_dispatches_no_calls() {
        let backend = FakeBackend::new();
        let probe = backend.clone();
        let mut inj = Injector::new(backend);
        inj.inject("", &mk_target(ChunkingPolicy::native_default())).unwrap();
        assert_eq!(probe.call_count(), 0);
    }

    fn mk_target(policy: ChunkingPolicy) -> FocusTarget {
        FocusTarget {
            os_window_id: 1,
            foreground_window_id: 1,
            process_id: 99,
            secondary_identity: medasr_types::SecondaryIdentity::WindowsThreadId(99),
            bundle_id_hash: None,
            chunking_policy: policy,
        }
    }
}
