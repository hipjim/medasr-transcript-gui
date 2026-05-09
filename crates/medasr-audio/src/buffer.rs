//! Recording-buffer primitive.
//!
//! `RecordingBuilder` accumulates resampled mono i16 @ 16 kHz samples into a
//! `SecureBuffer<i16>`, enforces the recording cap (R10), and produces a
//! warn-event at `WARN_SECS`. The orchestrator subscribes to those events.

use medasr_secure_buffer::SecureBuffer;
use medasr_types::{TARGET_SAMPLE_RATE_HZ, RECORDING_CAP_SECS, RECORDING_WARN_SECS};

/// Maximum samples (16 kHz mono) we'll ever hold in the recording buffer.
pub const CAP_SAMPLES: usize = (TARGET_SAMPLE_RATE_HZ as usize) * (RECORDING_CAP_SECS as usize);
/// Threshold sample count at which the warn-event is emitted.
pub const WARN_SAMPLES: usize = (TARGET_SAMPLE_RATE_HZ as usize) * (RECORDING_WARN_SECS as usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingOutcome {
    /// Recording stopped before the cap; buffer holds N samples.
    Completed,
    /// Recording was truncated at the cap.
    CapExceeded,
}

/// Single-recording accumulator. Not thread-safe; the orchestrator owns
/// it on the consumer side of the SPSC ring.
pub struct RecordingBuilder {
    buf: SecureBuffer<i16>,
    len: usize,
    warn_emitted: bool,
}

impl RecordingBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: SecureBuffer::with_capacity(CAP_SAMPLES),
            len: 0,
            warn_emitted: false,
        }
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub fn samples(&self) -> &[i16] { &self.buf[..self.len] }
    pub fn warn_emitted(&self) -> bool { self.warn_emitted }

    /// Append samples up to the cap. Returns the number of samples actually
    /// stored and whether the warn threshold crossed during this push.
    pub fn push(&mut self, src: &[i16]) -> PushResult {
        let space = CAP_SAMPLES.saturating_sub(self.len);
        let n = src.len().min(space);
        self.buf.as_mut_slice()[self.len..self.len + n].copy_from_slice(&src[..n]);
        self.len += n;

        let warn_crossed = !self.warn_emitted && self.len >= WARN_SAMPLES;
        if warn_crossed {
            self.warn_emitted = true;
        }
        PushResult {
            stored: n,
            cap_reached: self.len >= CAP_SAMPLES,
            warn_crossed,
        }
    }

    /// Consume the builder, yielding the underlying buffer truncated to
    /// the actual recorded length. The returned `SecureBuffer` carries the
    /// PHI residency guarantees of `medasr-secure-buffer`.
    #[must_use]
    pub fn finish(self) -> SecureBuffer<i16> {
        // We over-allocated to CAP_SAMPLES; copy the live prefix into a
        // right-sized SecureBuffer so the caller doesn't have to remember
        // the truncation point.
        let mut out = SecureBuffer::<i16>::with_capacity(self.len);
        out.as_mut_slice().copy_from_slice(&self.buf[..self.len]);
        out
    }
}

impl Default for RecordingBuilder {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone, Copy)]
pub struct PushResult {
    pub stored: usize,
    pub cap_reached: bool,
    pub warn_crossed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_finishes_to_empty() {
        let b = RecordingBuilder::new();
        let out = b.finish();
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn push_within_cap_records_all_samples() {
        let mut b = RecordingBuilder::new();
        let src = vec![42_i16; 16_000]; // 1 sec
        let r = b.push(&src);
        assert_eq!(r.stored, 16_000);
        assert!(!r.cap_reached);
        assert!(!r.warn_crossed);
        assert_eq!(b.len(), 16_000);
    }

    #[test]
    fn push_crossing_warn_threshold_emits_warn_once() {
        let mut b = RecordingBuilder::new();
        // 60 s of audio → exactly at warn threshold.
        let src = vec![1_i16; WARN_SAMPLES];
        let r = b.push(&src);
        assert!(r.warn_crossed);
        // Subsequent push should not re-emit warn.
        let src2 = vec![1_i16; 16_000];
        let r2 = b.push(&src2);
        assert!(!r2.warn_crossed);
    }

    #[test]
    fn push_exceeding_cap_truncates() {
        let mut b = RecordingBuilder::new();
        // 95 s of audio → must truncate at 90 s = CAP_SAMPLES.
        let src = vec![1_i16; CAP_SAMPLES + 16_000 * 5];
        let r = b.push(&src);
        assert_eq!(r.stored, CAP_SAMPLES);
        assert!(r.cap_reached);
        assert_eq!(b.len(), CAP_SAMPLES);
        let out = b.finish();
        assert_eq!(out.len(), CAP_SAMPLES);
    }

    #[test]
    fn finish_yields_truncated_buffer() {
        let mut b = RecordingBuilder::new();
        b.push(&vec![7_i16; 1234]);
        let out = b.finish();
        assert_eq!(out.len(), 1234);
        assert_eq!(out.as_slice()[0], 7);
        assert_eq!(out.as_slice()[1233], 7);
    }
}
