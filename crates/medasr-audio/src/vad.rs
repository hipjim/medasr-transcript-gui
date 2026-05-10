//! Energy-based VAD.
//!
//! Empty / non-speech audio causes Conformer-CTC models to hallucinate
//! ("thank you for watching", "subscribe to the channel", etc.). A simple
//! voiced-energy gate is the cheapest defense: we sum the squared amplitude
//! over short frames and require >= `min_voiced_ms` of frames above
//! `energy_threshold` before the buffer is forwarded to inference.
//!
//! This is intentionally simple. If the radiology fixture shows the
//! threshold is too tight (<5% false-quiet) we escalate to webrtc-vad and
//! then silero-vad per the plan, but only if the simpler approach does not
//! meet the bar.

use medasr_types::TARGET_SAMPLE_RATE_HZ;

#[derive(Debug, Clone, Copy)]
pub struct VadParams {
    pub frame_ms: u16,
    /// RMS energy threshold in [0, 1]. Empirical default: ~0.01 (very
    /// permissive). Hospital reading-room ambient noise typically lands
    /// well below this, while voice exceeds 0.05 routinely.
    pub energy_threshold: f32,
    /// Minimum cumulative voiced duration before we declare "speech".
    pub min_voiced_ms: u16,
}

impl Default for VadParams {
    fn default() -> Self {
        Self {
            frame_ms: 20,
            // 0.01 was too tight against typical condenser mic levels in
            // quiet rooms; lowered to 0.003 (≈ -50 dBFS RMS) which still
            // sits comfortably above background hum on dev hardware.
            energy_threshold: 0.003,
            min_voiced_ms: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    Speech,
    NoSpeech,
}

pub struct EnergyVad {
    params: VadParams,
}

impl EnergyVad {
    pub fn new(params: VadParams) -> Self {
        Self { params }
    }

    pub fn classify(&self, samples_16k_i16: &[i16]) -> VadDecision {
        if samples_16k_i16.is_empty() {
            return VadDecision::NoSpeech;
        }
        let frame_len = (TARGET_SAMPLE_RATE_HZ as usize * self.params.frame_ms as usize) / 1000;
        if frame_len == 0 {
            return VadDecision::NoSpeech;
        }

        let mut voiced_frames: u32 = 0;
        let mut max_rms: f32 = 0.0;
        for chunk in samples_16k_i16.chunks(frame_len) {
            let rms = rms_normalized(chunk);
            if rms > max_rms {
                max_rms = rms;
            }
            if rms >= self.params.energy_threshold {
                voiced_frames += 1;
            }
        }
        let voiced_ms = voiced_frames * u32::from(self.params.frame_ms);
        let decision = if voiced_ms >= u32::from(self.params.min_voiced_ms) {
            VadDecision::Speech
        } else {
            VadDecision::NoSpeech
        };
        tracing::info!(
            "vad: samples={} max_rms={:.4} threshold={:.4} voiced_ms={} -> {:?}",
            samples_16k_i16.len(),
            max_rms,
            self.params.energy_threshold,
            voiced_ms,
            decision,
        );
        decision
    }
}

fn rms_normalized(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let scale = f32::from(i16::MAX);
    let sum_sq: f64 = frame
        .iter()
        .map(|&s| {
            let n = f32::from(s) / scale;
            f64::from(n * n)
        })
        .sum();
    let mean = sum_sq / frame.len() as f64;
    (mean.sqrt()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(secs: f32) -> Vec<i16> {
        let n = (TARGET_SAMPLE_RATE_HZ as f32 * secs) as usize;
        vec![0_i16; n]
    }

    fn sine(secs: f32, freq_hz: f32, amplitude: f32) -> Vec<i16> {
        let n = (TARGET_SAMPLE_RATE_HZ as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / TARGET_SAMPLE_RATE_HZ as f32;
                let v = (t * freq_hz * std::f32::consts::TAU).sin() * amplitude;
                (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
            })
            .collect()
    }

    #[test]
    fn pure_silence_is_no_speech() {
        let vad = EnergyVad::new(VadParams::default());
        assert_eq!(vad.classify(&silence(1.0)), VadDecision::NoSpeech);
    }

    #[test]
    fn one_second_of_loud_tone_is_speech() {
        let vad = EnergyVad::new(VadParams::default());
        let buf = sine(1.0, 220.0, 0.3);
        assert_eq!(vad.classify(&buf), VadDecision::Speech);
    }

    #[test]
    fn brief_blip_under_min_voiced_ms_is_no_speech() {
        // 40 ms blip < default 100 ms threshold.
        let vad = EnergyVad::new(VadParams::default());
        let buf = sine(0.04, 220.0, 0.3);
        assert_eq!(vad.classify(&buf), VadDecision::NoSpeech);
    }

    #[test]
    fn empty_input_is_no_speech() {
        let vad = EnergyVad::new(VadParams::default());
        assert_eq!(vad.classify(&[]), VadDecision::NoSpeech);
    }
}
