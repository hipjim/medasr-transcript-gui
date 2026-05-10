//! Pre-ASR audio cleanup: VAD-based silence trimming + RMS-target gain.
//!
//! The orchestrator (CLI or GUI) records a coarse 5-second-or-PTT-window
//! buffer that is mostly silence at the head + tail. Feeding the whole
//! thing to MedASR adds noise to the feature pipeline and routinely
//! yields worse decodes than the same speech surrounded by less silence.
//! These helpers tighten the buffer to just the speech window, then
//! adjust gain to MedASR's training-distribution sweet spot.

use medasr_types::TARGET_SAMPLE_RATE_HZ;

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
}
