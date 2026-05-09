//! Resample arbitrary sample rate / channel count down to mono i16 @ 16 kHz.
//!
//! The orchestrator chooses to buffer-and-resample-once at hotkey-release
//! rather than streaming-per-callback because (a) MedASR is offline-batch
//! anyway, (b) we keep the audio callback as cheap as possible to avoid
//! sample drops on busy hospital workstations.

use rubato::{Resampler, FftFixedIn};
use thiserror::Error;

use medasr_types::TARGET_SAMPLE_RATE_HZ;

#[derive(Debug, Error)]
pub enum ResampleError {
    #[error("resampler init: {0}")]
    Init(String),
    #[error("resample step: {0}")]
    Process(String),
    #[error("zero channels — invalid input")]
    ZeroChannels,
}

/// Resample interleaved f32 samples (any rate, any channel count) to mono
/// i16 @ 16 kHz suitable for sherpa-onnx MedASR-CTC input.
///
/// Channel mixdown is a simple mean across channels. For radiology dictation
/// where the input is a single headset mic this is functionally identity.
pub fn resample_to_16k_mono(
    samples_interleaved: &[f32],
    src_rate: u32,
    channels: u16,
) -> Result<Vec<i16>, ResampleError> {
    if channels == 0 {
        return Err(ResampleError::ZeroChannels);
    }
    if samples_interleaved.is_empty() {
        return Ok(Vec::new());
    }

    // 1. Mixdown to mono.
    let frames = samples_interleaved.len() / channels as usize;
    let mut mono = Vec::with_capacity(frames);
    if channels == 1 {
        mono.extend_from_slice(samples_interleaved);
    } else {
        for f in 0..frames {
            let start = f * channels as usize;
            let end = start + channels as usize;
            let mean: f32 = samples_interleaved[start..end].iter().copied().sum::<f32>()
                / channels as f32;
            mono.push(mean);
        }
    }

    // 2. Resample if needed.
    let resampled = if src_rate == TARGET_SAMPLE_RATE_HZ {
        mono
    } else {
        let chunk = 1024.min(mono.len()).max(1);
        let mut resampler =
            FftFixedIn::<f32>::new(src_rate as usize, TARGET_SAMPLE_RATE_HZ as usize, chunk, 2, 1)
                .map_err(|e| ResampleError::Init(e.to_string()))?;

        let mut input_pos = 0usize;
        let chunk_in = resampler.input_frames_next();
        let mut output: Vec<f32> = Vec::with_capacity(
            mono.len() * TARGET_SAMPLE_RATE_HZ as usize / src_rate as usize + chunk_in,
        );

        while input_pos + chunk_in <= mono.len() {
            let buf = &mono[input_pos..input_pos + chunk_in];
            let out = resampler
                .process(&[buf], None)
                .map_err(|e| ResampleError::Process(e.to_string()))?;
            output.extend_from_slice(&out[0]);
            input_pos += chunk_in;
        }
        // Tail: pad the last frame with zeros so we don't drop trailing audio.
        if input_pos < mono.len() {
            let mut tail = mono[input_pos..].to_vec();
            tail.resize(chunk_in, 0.0);
            let out = resampler
                .process(&[tail], None)
                .map_err(|e| ResampleError::Process(e.to_string()))?;
            output.extend_from_slice(&out[0]);
        }
        output
    };

    // 3. Convert f32 [-1,1] -> i16 [-32768, 32767], clamped.
    Ok(resampled
        .into_iter()
        .map(|s| {
            let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round();
            v as i16
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_already_16k_mono() {
        let mono: Vec<f32> = (0..16_000).map(|i| (i as f32 / 16_000.0).sin()).collect();
        let out = resample_to_16k_mono(&mono, 16_000, 1).unwrap();
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn downsamples_48k_mono_to_16k() {
        // 1 second of audio at 48 kHz.
        let mono: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48_000.0 * 220.0).sin()).collect();
        let out = resample_to_16k_mono(&mono, 48_000, 1).unwrap();
        // Expect ~16 000 samples (rubato adds a small tail).
        let diff = (out.len() as i64 - 16_000).abs();
        assert!(
            diff < 1024,
            "expected ~16000 samples, got {} (diff {diff})",
            out.len()
        );
    }

    #[test]
    fn mixdown_stereo_to_mono() {
        // 2 frames of stereo silence + signal.
        let stereo = vec![0.5_f32, -0.5, 0.5, -0.5];
        let out = resample_to_16k_mono(&stereo, 16_000, 2).unwrap();
        // Mixed-down mean is 0; converted to i16 is 0.
        assert_eq!(out, vec![0_i16, 0]);
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = resample_to_16k_mono(&[], 16_000, 1).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn rejects_zero_channels() {
        let err = resample_to_16k_mono(&[0.0], 16_000, 0).unwrap_err();
        assert!(matches!(err, ResampleError::ZeroChannels));
    }
}
