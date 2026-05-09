use std::time::Duration;

/// Why a recording was aborted (no error toast — system or user
/// interrupted; expected control flow). Mirrors the plan's `Aborted` vs
/// `Error` distinction so tests can assert which path fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// User let go of the hotkey but VAD found no speech.
    NoSpeechDetected,
    /// Recording cap (90 s) reached.
    CapExceeded,
    /// OS sleep / lid close while recording.
    OsSleep,
    /// Microphone disconnected during recording.
    MicrophoneLost,
    /// User cancelled mid-recording.
    UserCancelled,
}

/// Why an Error transition fired (toast required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    HotkeyConflict,
    PermissionDenied,
    ModelMissing,
    InferenceFailed,
    TargetWindowLost,
    SecureInputBlocked,
    SecureInputBlockedMidStream,
    TlsPinMismatch,
    WaylandNotSupported,
    Unknown,
}

/// External events the orchestrator delivers to the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    EulaAccepted,
    EulaDeclined,
    PermissionsGranted,
    ModelDownloadStarted,
    ModelDownloadComplete,
    ModelVerifyOk,
    WarmupComplete,

    HotkeyPressed,
    /// Hotkey released after `held` time. Used for the press-during-warming
    /// queue heuristic.
    HotkeyReleased { held: Duration },

    /// Audio capture has produced a non-empty SecureBuffer. The
    /// orchestrator owns the SecureBuffer side-channel separately; the
    /// event itself only carries metadata.
    AudioReady { samples: usize },
    /// VAD said no-speech.
    NoSpeech,
    /// ASR worker returned a transcript (orchestrator holds the actual
    /// SecureBuffer<u8>).
    TranscriptReady,
    /// Injection successfully flushed all chunks.
    InjectionComplete,

    Aborted(AbortReason),
    Error(ErrorClass),
    /// User dismissed the error / abort toast.
    Acknowledged,

    /// OS sent a sleep / wake notification.
    OsSleeping,
    OsWaking,
}
