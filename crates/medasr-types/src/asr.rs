use std::time::Duration;

/// Result of a single offline batch transcription.
///
/// In production the `text` field is held in a `SecureBuffer<u8>` rather than
/// a `String`. This struct is what the orchestrator hands to the
/// post-processor at the seam.
#[derive(Debug, Clone)]
pub struct AsrResult {
    pub text: String,
    pub inference_latency: Duration,
    /// Length of the audio that was transcribed. The orchestrator records
    /// this in audit-log duration buckets.
    pub audio_duration: Duration,
}
