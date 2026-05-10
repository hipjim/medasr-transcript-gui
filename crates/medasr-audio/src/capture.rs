//! Cross-platform audio capture via `cpal`.
//!
//! Architecture: the audio callback runs on cpal's OS-priority thread and
//! must do as little work as possible — it pushes raw f32 samples + the
//! captured stream config into a lock-free SPSC ring (`rtrb::Producer`).
//! The orchestrator drains the consumer side on hotkey-release and runs
//! the (heavier) resample + VAD + handoff on its own task.
//!
//! Default-input-device is queried at recording start (not cached) so that
//! a mic hot-swap between recordings is handled transparently.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rtrb::{Producer, RingBuffer};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("no default input device available")]
    NoInputDevice,
    #[error("default config: {0}")]
    DefaultConfig(String),
    #[error("build stream: {0}")]
    BuildStream(String),
    #[error("play stream: {0}")]
    PlayStream(String),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),
}

/// Minimum config the orchestrator needs to drain the SPSC ring.
#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Holds the live cpal `Stream` plus the SPSC consumer end of the ring.
/// Drop the `AudioCapture` to stop capture (the cpal stream pauses on
/// drop).
pub struct AudioCapture {
    pub config: CaptureConfig,
    pub consumer: rtrb::Consumer<f32>,
    pub overflow_flag: Arc<AtomicBool>,
    /// Most-recent in-callback peak amplitude, scaled to 0..=10000 (i.e.
    /// integer hundredths of a percent of full scale). The audio
    /// callback writes; UI/orchestrator polls.
    pub peak_centi_pct: Arc<AtomicU32>,
    _stream: Stream,
}

impl AudioCapture {
    /// Read the most recent peak level as a 0.0..=1.0 fraction.
    pub fn current_peak_fraction(&self) -> f32 {
        self.peak_centi_pct.load(Ordering::Relaxed) as f32 / 10_000.0
    }
}

/// One available input device, surfaced for the UI device picker.
#[derive(Clone)]
pub struct InputDevice {
    pub name: String,
    pub device: Device,
}

impl std::fmt::Debug for InputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InputDevice {{ name: {:?} }}", self.name)
    }
}

pub fn list_input_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| {
            it.map(|d| InputDevice {
                name: d.name().unwrap_or_else(|_| "<unnamed>".into()),
                device: d,
            })
            .collect()
        })
        .unwrap_or_default()
}

pub fn default_input_device() -> Option<InputDevice> {
    let host = cpal::default_host();
    host.default_input_device().map(|d| InputDevice {
        name: d.name().unwrap_or_else(|_| "<unnamed>".into()),
        device: d,
    })
}

/// Default ring capacity: ~250 ms @ 192 kHz × 8 channels (worst-case
/// hospital-grade audio interface). The orchestrator drains continuously
/// during recording so the steady state is small.
const RING_CAPACITY: usize = 192_000 * 2 / 4; // ~96 000 f32 samples

pub fn start() -> Result<AudioCapture, CaptureError> {
    let device = default_input_device().ok_or(CaptureError::NoInputDevice)?;
    start_with_device(&device)
}

pub fn start_with_device(device: &InputDevice) -> Result<AudioCapture, CaptureError> {
    let supported = device
        .device
        .default_input_config()
        .map_err(|e| CaptureError::DefaultConfig(e.to_string()))?;
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.config();
    let cfg = CaptureConfig {
        sample_rate: stream_config.sample_rate.0,
        channels: stream_config.channels,
    };

    let (mut producer, consumer) = RingBuffer::<f32>::new(RING_CAPACITY);
    let overflow_flag = Arc::new(AtomicBool::new(false));
    let overflow_flag_clb = Arc::clone(&overflow_flag);
    let peak_centi_pct = Arc::new(AtomicU32::new(0));
    let peak_clb = Arc::clone(&peak_centi_pct);

    let stream = build_stream(
        &device.device,
        &stream_config,
        sample_format,
        move |samples: &[f32]| {
            // Update the peak meter (integer hundredths of a percent of
            // full scale, exponential-decay smoothing).
            let mut peak: f32 = 0.0;
            for &s in samples {
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
            let prev = peak_clb.load(Ordering::Relaxed) as f32 / 10_000.0;
            let smoothed = if peak > prev {
                peak
            } else {
                prev * 0.85 + peak * 0.15
            };
            peak_clb.store((smoothed * 10_000.0) as u32, Ordering::Relaxed);
            push_into_ring(&mut producer, samples, &overflow_flag_clb);
        },
    )?;
    stream
        .play()
        .map_err(|e| CaptureError::PlayStream(e.to_string()))?;

    Ok(AudioCapture {
        config: cfg,
        consumer,
        overflow_flag,
        peak_centi_pct,
        _stream: stream,
    })
}

fn push_into_ring(producer: &mut Producer<f32>, samples: &[f32], overflow_flag: &AtomicBool) {
    for &s in samples {
        if producer.push(s).is_err() {
            // Full — orchestrator has not drained fast enough. We drop the
            // sample (audio gap rather than blocking the OS audio thread).
            // The consumer surfaces the overflow flag once.
            overflow_flag.store(true, Ordering::Relaxed);
            return;
        }
    }
}

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    fmt: SampleFormat,
    mut sink: impl FnMut(&[f32]) + Send + 'static,
) -> Result<Stream, CaptureError> {
    let err_cb = |e: cpal::StreamError| {
        warn!("cpal stream error: {e}");
    };
    let stream = match fmt {
        SampleFormat::F32 => device
            .build_input_stream(config, move |data: &[f32], _| sink(data), err_cb, None)
            .map_err(|e| CaptureError::BuildStream(e.to_string()))?,
        SampleFormat::I16 => {
            let scale = 1.0 / f32::from(i16::MAX);
            let mut tmp: Vec<f32> = Vec::with_capacity(2048);
            device
                .build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        tmp.clear();
                        tmp.extend(data.iter().map(|&s| f32::from(s) * scale));
                        sink(&tmp);
                    },
                    err_cb,
                    None,
                )
                .map_err(|e| CaptureError::BuildStream(e.to_string()))?
        }
        SampleFormat::U16 => {
            let mut tmp: Vec<f32> = Vec::with_capacity(2048);
            device
                .build_input_stream(
                    config,
                    move |data: &[u16], _| {
                        tmp.clear();
                        tmp.extend(data.iter().map(|&s| {
                            // u16 -> f32 in [-1, 1].
                            let centered = i32::from(s) - 32_768;
                            centered as f32 / 32_768.0
                        }));
                        sink(&tmp);
                    },
                    err_cb,
                    None,
                )
                .map_err(|e| CaptureError::BuildStream(e.to_string()))?
        }
        other => return Err(CaptureError::UnsupportedFormat(other)),
    };
    Ok(stream)
}
