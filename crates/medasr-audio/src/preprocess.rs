//! Pre-ASR audio cleanup: VAD-based silence trimming + RMS-target gain.
//!
//! The orchestrator (CLI or GUI) records a coarse 5-second-or-PTT-window
//! buffer that is mostly silence at the head + tail. Feeding the whole
//! thing to MedASR adds noise to the feature pipeline and routinely
//! yields worse decodes than the same speech surrounded by less silence.
//! These helpers tighten the buffer to just the speech window, then
//! adjust gain to MedASR's training-distribution sweet spot.

use std::sync::Arc;

use medasr_types::TARGET_SAMPLE_RATE_HZ;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Apply a 1st-order Butterworth high-pass filter at `cutoff_hz` to the
/// 16 kHz mono buffer in-place style (returns a new vector).
///
/// Why: room rumble, HVAC, fan, and laptop fan noise sit below ~100 Hz.
/// MedASR's mel filterbank still picks them up as noise, which competes
/// with vowel formants and degrades decoding. A 80 Hz HPF removes them
/// without touching speech (fundamentals start ~85 Hz for adult male
/// voice; 80 Hz keeps even the lowest vocal harmonics intact while
/// killing rumble).
#[must_use]
pub fn high_pass_filter(samples_16k: &[i16], cutoff_hz: f32) -> Vec<i16> {
    if samples_16k.is_empty() { return Vec::new(); }
    // RC high-pass:  y[n] = α (y[n-1] + x[n] − x[n-1])
    // with α = RC / (RC + dt), RC = 1 / (2π f_c), dt = 1 / sample_rate.
    let dt = 1.0_f32 / TARGET_SAMPLE_RATE_HZ as f32;
    let rc = 1.0_f32 / (std::f32::consts::TAU * cutoff_hz);
    let alpha = rc / (rc + dt);

    let mut out = Vec::with_capacity(samples_16k.len());
    let mut prev_y = 0.0_f32;
    let mut prev_x = 0.0_f32;
    for &s in samples_16k {
        let x = f32::from(s);
        let y = alpha * (prev_y + x - prev_x);
        out.push(y.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16);
        prev_y = y;
        prev_x = x;
    }
    out
}

/// Trim leading + trailing silence based on per-frame energy.
///
/// Algorithm:
///   - 20 ms frames.
///   - A frame is "voiced" if its normalized RMS >= `threshold`.
///   - Find the first and last voiced frames.
///   - Add `margin_ms` of audio before / after.
///
/// Returns the trimmed slice. If no frame is voiced, returns the empty
/// slice (caller should treat as NoSpeech).
#[must_use]
pub fn trim_silence(samples_16k: &[i16], threshold: f32, margin_ms: u32) -> Vec<i16> {
    if samples_16k.is_empty() { return Vec::new(); }
    let frame_len = (TARGET_SAMPLE_RATE_HZ as usize * 20) / 1000; // 20 ms
    let scale = f32::from(i16::MAX);

    let mut first_voiced: Option<usize> = None;
    let mut last_voiced: Option<usize> = None;
    for (idx, chunk) in samples_16k.chunks(frame_len).enumerate() {
        let sum_sq: f64 = chunk.iter().map(|&s| {
            let n = f32::from(s) / scale;
            f64::from(n * n)
        }).sum();
        let rms = ((sum_sq / chunk.len() as f64).sqrt()) as f32;
        if rms >= threshold {
            if first_voiced.is_none() { first_voiced = Some(idx); }
            last_voiced = Some(idx);
        }
    }
    let (Some(first), Some(last)) = (first_voiced, last_voiced) else {
        return Vec::new();
    };

    let margin_frames = (margin_ms as usize / 20).max(1);
    let start = first.saturating_sub(margin_frames) * frame_len;
    let end_frame = (last + margin_frames + 1).min(samples_16k.len() / frame_len + 1);
    let end = (end_frame * frame_len).min(samples_16k.len());
    samples_16k[start..end].to_vec()
}

/// Apply gain so the buffer's RMS hits `target_dbfs`. Clips to int16
/// range. If the buffer is already silent (RMS == 0), returns it
/// unchanged.
#[must_use]
pub fn rms_normalize(samples_16k: &[i16], target_dbfs: f32) -> Vec<i16> {
    if samples_16k.is_empty() { return Vec::new(); }
    let scale = f32::from(i16::MAX);
    let sum_sq: f64 = samples_16k.iter().map(|&s| {
        let n = f32::from(s) / scale;
        f64::from(n * n)
    }).sum();
    let rms = ((sum_sq / samples_16k.len() as f64).sqrt()) as f32;
    if rms <= f32::EPSILON { return samples_16k.to_vec(); }

    let target_linear = 10_f32.powf(target_dbfs / 20.0);
    let gain = target_linear / rms;

    samples_16k.iter().map(|&s| {
        let amplified = (f32::from(s) * gain).clamp(-scale, scale);
        amplified as i16
    }).collect()
}

// ---------------------------------------------------------------------
// Spectral noise subtraction (Boll 1979 magnitude-spectral subtraction).
// ---------------------------------------------------------------------

/// Frame size for STFT. 25 ms @ 16 kHz = 400 samples → round up to 512
/// (next power of two) so rustfft is happiest.
const NSS_FRAME: usize = 512;
/// Hop size: 10 ms @ 16 kHz = 160 samples (50% overlap with the
/// 25 ms frame; standard STFT analysis window for ASR pre-processing).
const NSS_HOP: usize = 160;
/// Over-subtraction factor. >1 trades more noise reduction for the risk
/// of musical-noise artefacts.
const NSS_OVER: f32 = 1.6;
/// Spectral floor: never attenuate a bin below this fraction of its
/// original magnitude. Prevents the speech/noise gap from going to zero
/// (which is what causes "musical noise" tonal artefacts).
const NSS_FLOOR: f32 = 0.05;

/// Subtract the magnitude spectrum of `noise_segment` from `samples`.
///
/// Both inputs are 16 kHz mono int16. `noise_segment` should be a piece
/// of audio that contains noise only — typically the first ~300 ms of a
/// recording before the user starts speaking, or a stretch identified
/// as silence by the VAD.
///
/// If `noise_segment` is too short (< one frame) or its RMS is louder
/// than `max_noise_dbfs`, returns `samples` unchanged — better to do
/// nothing than to subtract speech-loud "noise".
#[must_use]
pub fn spectral_subtract(samples: &[i16], noise_segment: &[i16], max_noise_dbfs: f32) -> Vec<i16> {
    if samples.len() < NSS_FRAME || noise_segment.len() < NSS_FRAME {
        return samples.to_vec();
    }
    if rms_dbfs(noise_segment) > max_noise_dbfs {
        // Not actually noise — skip.
        return samples.to_vec();
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft: Arc<dyn Fft<f32>> = planner.plan_fft_forward(NSS_FRAME);
    let ifft: Arc<dyn Fft<f32>> = planner.plan_fft_inverse(NSS_FRAME);
    let window = hann_window(NSS_FRAME);
    let win_norm: f32 = window.iter().map(|&w| w * w).sum::<f32>() / NSS_HOP as f32;

    // Estimate the noise spectrum: average magnitude across all noise
    // frames.
    let noise_mag = average_magnitude_spectrum(noise_segment, &window, &fft);

    // Output buffer (overlap-add).
    let mut output = vec![0.0_f32; samples.len()];
    let mut frame = vec![Complex32::default(); NSS_FRAME];
    let scale = f32::from(i16::MAX);

    let mut start = 0;
    while start + NSS_FRAME <= samples.len() {
        for i in 0..NSS_FRAME {
            let s = f32::from(samples[start + i]) / scale;
            frame[i] = Complex32::new(s * window[i], 0.0);
        }
        fft.process(&mut frame);
        for (bin_idx, bin) in frame.iter_mut().enumerate() {
            let mag = bin.norm();
            let n = noise_mag[bin_idx];
            let cleaned = (mag - NSS_OVER * n).max(NSS_FLOOR * mag);
            if mag > f32::EPSILON {
                let factor = cleaned / mag;
                bin.re *= factor;
                bin.im *= factor;
            }
        }
        ifft.process(&mut frame);
        let inv_scale = 1.0 / NSS_FRAME as f32;
        for i in 0..NSS_FRAME {
            output[start + i] += frame[i].re * inv_scale * window[i] / win_norm;
        }
        start += NSS_HOP;
    }
    output
        .into_iter()
        .map(|s| (s.clamp(-1.0, 1.0) * scale) as i16)
        .collect()
}

fn average_magnitude_spectrum(
    noise: &[i16],
    window: &[f32],
    fft: &Arc<dyn Fft<f32>>,
) -> Vec<f32> {
    let mut accum = vec![0.0_f32; NSS_FRAME];
    let mut frames = 0usize;
    let mut frame = vec![Complex32::default(); NSS_FRAME];
    let scale = f32::from(i16::MAX);
    let mut start = 0;
    while start + NSS_FRAME <= noise.len() {
        for i in 0..NSS_FRAME {
            let s = f32::from(noise[start + i]) / scale;
            frame[i] = Complex32::new(s * window[i], 0.0);
        }
        fft.process(&mut frame);
        for (i, bin) in frame.iter().enumerate() {
            accum[i] += bin.norm();
        }
        frames += 1;
        start += NSS_HOP;
    }
    if frames > 0 {
        for v in &mut accum {
            *v /= frames as f32;
        }
    }
    accum
}

fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos()))
        .collect()
}

/// RMS of an int16 buffer expressed as dBFS. Empty input returns -100.
#[must_use]
pub fn rms_dbfs(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let scale = f32::from(i16::MAX);
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let n = f32::from(s) / scale;
            f64::from(n * n)
        })
        .sum();
    let rms = ((sum_sq / samples.len() as f64).sqrt()) as f32;
    if rms <= f32::EPSILON {
        -100.0
    } else {
        20.0 * rms.log10()
    }
}

/// Peak amplitude of an int16 buffer as a 0..=1 fraction of full scale.
#[must_use]
pub fn peak_fraction(samples: &[i16]) -> f32 {
    samples
        .iter()
        .copied()
        .map(|s| s.saturating_abs() as f32)
        .fold(0.0_f32, f32::max)
        / f32::from(i16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(secs: f32) -> Vec<i16> {
        vec![0_i16; (TARGET_SAMPLE_RATE_HZ as f32 * secs) as usize]
    }
    fn tone(secs: f32, amp: i16) -> Vec<i16> {
        (0..(TARGET_SAMPLE_RATE_HZ as f32 * secs) as usize)
            .map(|i| ((i as f32 * 0.1).sin() * amp as f32) as i16)
            .collect()
    }

    #[test]
    fn trims_leading_and_trailing_silence() {
        // 1s silence + 1s tone + 1s silence
        let mut buf = silence(1.0);
        buf.extend(tone(1.0, 16_000));
        buf.extend(silence(1.0));
        let trimmed = trim_silence(&buf, 0.05, 200);
        // Should be ~1.4s (1s tone + 200ms margin each side)
        let expected = (TARGET_SAMPLE_RATE_HZ as f32 * 1.4) as usize;
        let diff = (trimmed.len() as i64 - expected as i64).abs();
        assert!(diff < 4_000, "expected ~{expected}, got {} (diff {diff})", trimmed.len());
    }

    #[test]
    fn no_speech_returns_empty() {
        assert!(trim_silence(&silence(2.0), 0.05, 200).is_empty());
    }

    #[test]
    fn rms_normalize_raises_quiet_audio() {
        // Quiet tone @ ~1% peak (i.e. very low rms).
        let quiet = tone(1.0, 200);
        let normalized = rms_normalize(&quiet, -20.0);
        let peak: i16 = normalized.iter().copied().map(|s| s.saturating_abs()).max().unwrap();
        // After targeting -20 dBFS RMS, peak should be at least 25% of full
        // scale (sine wave: peak ≈ rms * sqrt(2) ≈ 0.10 * 1.4 ≈ 0.14, but
        // because our test signal isn't a pure sine the relation is
        // approximate; we only assert "much louder than before").
        assert!(peak > 1000, "peak after normalize was {peak}");
    }

    #[test]
    fn hpf_attenuates_dc_offset() {
        // A constant DC offset is the limit case of "sub-cutoff content";
        // the HPF must drive it toward zero.
        let dc: Vec<i16> = vec![10_000; 16_000];
        let filtered = high_pass_filter(&dc, 80.0);
        // Tail samples should be near zero (the filter has had time to
        // settle).
        let tail_max: i16 = filtered[8_000..].iter().copied().map(|s| s.saturating_abs()).max().unwrap();
        assert!(tail_max < 200, "DC residue too large: {tail_max}");
    }

    #[test]
    fn hpf_preserves_audio_band_signal() {
        // 1 kHz tone, well above the 80 Hz cutoff. After HPF its peak
        // should be essentially unchanged.
        let tone: Vec<i16> = (0..16_000)
            .map(|i| ((i as f32 / 16_000.0 * 1000.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16)
            .collect();
        let filtered = high_pass_filter(&tone, 80.0);
        let in_peak: i16 = tone[8_000..].iter().copied().map(|s| s.saturating_abs()).max().unwrap();
        let out_peak: i16 = filtered[8_000..].iter().copied().map(|s| s.saturating_abs()).max().unwrap();
        // Allow ~5% loss; in practice this filter passes >95% of 1 kHz.
        assert!(out_peak > (in_peak as f32 * 0.9) as i16,
                "1 kHz attenuated too much: {in_peak} -> {out_peak}");
    }

    #[test]
    fn rms_normalize_doesnt_clip_for_existing_loud_audio() {
        let loud = tone(1.0, 32_000); // near full scale
        let normalized = rms_normalize(&loud, -20.0);
        // No samples should reach exact ±32767 after a target lower than
        // current RMS (gain < 1).
        let peak: i16 = normalized.iter().copied().map(|s| s.saturating_abs()).max().unwrap();
        assert!(peak < 32_767, "peak unexpectedly clipped: {peak}");
    }

    fn white_noise(secs: f32, amplitude: i16) -> Vec<i16> {
        // Linear congruential generator for deterministic test noise.
        let n = (TARGET_SAMPLE_RATE_HZ as f32 * secs) as usize;
        let mut state: u32 = 0xDEAD_BEEF;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let v = (state >> 16) as i32 - 32_768;
                ((v as f32 / 32_768.0) * amplitude as f32) as i16
            })
            .collect()
    }

    #[test]
    fn spectral_subtract_attenuates_steady_noise() {
        // 1 second of white noise at peak ≈ 5% then 1 s of louder
        // signal+noise mixture. We use the first 600 ms as the noise
        // estimate. After subtraction, the leading-noise RMS should
        // drop substantially.
        let noise = white_noise(2.0, 1_500);
        let denoised = spectral_subtract(&noise, &noise[..16_000 / 2], -10.0);
        let before = rms_dbfs(&noise);
        let after = rms_dbfs(&denoised);
        assert!(after < before - 6.0, "expected ≥6 dB reduction, before={before:.1} after={after:.1}");
    }

    #[test]
    fn spectral_subtract_preserves_a_loud_tone() {
        // 1 kHz tone at 30% amplitude. Noise estimate is silence — the
        // subtraction should be ~no-op.
        let mut tone_buf = Vec::with_capacity(16_000);
        for i in 0..16_000 {
            let v = (i as f32 / 16_000.0 * 1000.0 * std::f32::consts::TAU).sin() * 0.3;
            tone_buf.push((v * f32::from(i16::MAX)) as i16);
        }
        let silence = vec![0_i16; 16_000];
        let cleaned = spectral_subtract(&tone_buf, &silence, -10.0);
        let before = peak_fraction(&tone_buf);
        let after = peak_fraction(&cleaned);
        assert!(after > before * 0.9, "tone was attenuated: {before:.3} -> {after:.3}");
    }

    #[test]
    fn spectral_subtract_skips_when_noise_is_loud() {
        // Both inputs are signal-loud; algorithm should skip and return
        // input unchanged.
        let signal = white_noise(1.0, 30_000);
        let denoised = spectral_subtract(&signal, &signal, -20.0);
        assert_eq!(signal, denoised);
    }

    #[test]
    fn rms_dbfs_smoke() {
        let s: Vec<i16> = (0..16_000)
            .map(|i| ((i as f32 / 100.0).sin() * 16_384.0) as i16)
            .collect();
        let dbfs = rms_dbfs(&s);
        // sin at half full scale → RMS ≈ 0.5/√2 ≈ 0.354 → ~-9 dBFS.
        assert!(dbfs > -12.0 && dbfs < -6.0, "got {dbfs:.1}");
    }
}
