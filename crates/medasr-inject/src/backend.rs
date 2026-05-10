use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend init: {0}")]
    Init(String),
    #[error("type call: {0}")]
    Type(String),
}

/// Cross-platform keystroke synthesis seam.
///
/// Implementations MUST:
///  - Type the string at the OS-level keyboard input layer (NOT the
///    clipboard).
///  - Block until the keystrokes are flushed to the focused window before
///    returning.
///
/// Note: backends are intentionally NOT `Send`. macOS's `CGEventSource`
/// (used by enigo) is not thread-safe; the orchestrator confines its
/// injector to a single dedicated thread and dispatches work to it via
/// mpsc — same shape as the audio and ASR workers.
pub trait KeystrokeBackend {
    /// Type the supplied string at the current focused window.
    fn type_unicode_string(&mut self, text: &str) -> Result<(), BackendError>;

    /// Force any buffered keystrokes to be delivered before returning.
    /// Default impl is a no-op (most backends auto-flush per call).
    fn flush(&mut self) -> Result<(), BackendError> {
        Ok(())
    }
}
