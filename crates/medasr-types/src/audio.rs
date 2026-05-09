/// Sample rate the ASR pipeline expects. Audio captured at the device
/// native rate is resampled to this in `medasr-audio`.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Hard recording cap (R10). The orchestrator stops capture at this point.
pub const RECORDING_CAP_SECS: u32 = 90;

/// Soft warning emitted via state-machine event before the cap. Gives the
/// user a chance to wrap up.
pub const RECORDING_WARN_SECS: u32 = 60;

/// PCM mono int16 samples at `TARGET_SAMPLE_RATE_HZ`.
///
/// In production this is allocated through `medasr-secure-buffer::SecureBuffer`
/// so that PHI-bearing audio is mlock'd and zeroed on drop. The plain `Vec`
/// alias here exists only as a transport type for tests and FFI seams; the
/// orchestrator never owns one.
pub type PcmInt16 = Vec<i16>;
