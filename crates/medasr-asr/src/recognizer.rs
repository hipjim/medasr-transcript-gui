//! `Asr` — single-threaded recognizer wrapper around `sherpa-onnx`.
//!
//! The orchestrator owns one of these on its dedicated ASR worker thread.
//! The struct is intentionally not `Send` from the caller's perspective: it
//! lives behind the worker (`worker.rs`) which moves it once at thread
//! start and never again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use medasr_secure_buffer::SecureBuffer;
use medasr_types::{AsrResult, TARGET_SAMPLE_RATE_HZ};
use sherpa_onnx::{
    OfflineMedAsrCtcModelConfig, OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Filesystem layout the recognizer expects. Populated by Unit 4 (model
/// fetch); for Unit 3 the orchestrator just provides the paths.
#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub model_int8_onnx: PathBuf,
    pub tokens_txt: PathBuf,
}

impl ModelPaths {
    /// Standard layout produced by Unit 4: a per-revision directory under
    /// `medasr-paths::model_cache_dir()/medasr-int8-<revision>/{model.int8.onnx, tokens.txt}`.
    pub fn from_dir(dir: &Path) -> Self {
        Self {
            model_int8_onnx: dir.join("model.int8.onnx"),
            tokens_txt: dir.join("tokens.txt"),
        }
    }
}

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("model file missing or unreadable: {0}")]
    ModelMissing(PathBuf),
    #[error("recognizer init failed (sherpa-onnx returned None — typically corrupt model)")]
    Init,
    #[error("decode failed: stream returned no result")]
    NoResult,
    #[error("cancelled")]
    Cancelled,
}

pub struct Asr {
    inner: OfflineRecognizer,
}

impl Asr {
    /// Build a recognizer from an on-disk model directory and run a 1-second
    /// silence warmup so the first real transcribe doesn't pay the
    /// page-in cost.
    pub fn new(paths: &ModelPaths, cancel: &CancellationToken) -> Result<Self, AsrError> {
        if !paths.model_int8_onnx.is_file() {
            return Err(AsrError::ModelMissing(paths.model_int8_onnx.clone()));
        }
        if !paths.tokens_txt.is_file() {
            return Err(AsrError::ModelMissing(paths.tokens_txt.clone()));
        }
        if cancel.is_cancelled() {
            return Err(AsrError::Cancelled);
        }

        let mut cfg = OfflineRecognizerConfig::default();
        cfg.model_config = OfflineModelConfig {
            medasr: OfflineMedAsrCtcModelConfig {
                model: Some(paths.model_int8_onnx.to_string_lossy().into_owned()),
            },
            tokens: Some(paths.tokens_txt.to_string_lossy().into_owned()),
            num_threads: num_threads(),
            debug: false,
            provider: Some("cpu".to_string()),
            ..Default::default()
        };
        cfg.decoding_method = Some("greedy_search".to_string());

        let inner = OfflineRecognizer::create(&cfg).ok_or(AsrError::Init)?;
        let me = Self { inner };

        // Warmup: 1 s of silence. Pages model weights and JIT'd kernels.
        let warmup_buf = SecureBuffer::<i16>::with_capacity(TARGET_SAMPLE_RATE_HZ as usize);
        let _ = me.transcribe(warmup_buf.as_slice(), cancel);
        info!("ASR warmup complete");
        Ok(me)
    }

    /// Run an offline batch transcribe on the input samples.
    ///
    /// `samples_16k_i16` MUST be mono int16 at 16 kHz (the contract enforced
    /// upstream by `medasr-audio::resample_to_16k_mono`).
    pub fn transcribe(
        &self,
        samples_16k_i16: &[i16],
        cancel: &CancellationToken,
    ) -> Result<AsrResult, AsrError> {
        if cancel.is_cancelled() {
            return Err(AsrError::Cancelled);
        }
        let started = Instant::now();
        let n = samples_16k_i16.len();
        let audio_duration =
            Duration::from_secs_f64(n as f64 / f64::from(TARGET_SAMPLE_RATE_HZ));

        // i16 -> f32 in [-1, 1] via SecureBuffer<f32> so the conversion
        // stays mlock'd / zero-on-drop. (sherpa-onnx C API will copy this
        // internally; the heap copy inside ONNX Runtime is the documented
        // residual gap.)
        let scale = 1.0_f32 / f32::from(i16::MAX);
        let mut f32_buf = SecureBuffer::<f32>::with_capacity(n);
        for (dst, &src) in f32_buf.as_mut_slice().iter_mut().zip(samples_16k_i16) {
            *dst = f32::from(src) * scale;
        }

        if cancel.is_cancelled() {
            return Err(AsrError::Cancelled);
        }
        let stream = self.inner.create_stream();
        stream.accept_waveform(TARGET_SAMPLE_RATE_HZ as i32, f32_buf.as_slice());

        if cancel.is_cancelled() {
            return Err(AsrError::Cancelled);
        }
        self.inner.decode(&stream);

        let res = stream.get_result().ok_or(AsrError::NoResult)?;
        let inference_latency = started.elapsed();
        debug!(
            "transcribe: {} ms inference for {} ms audio",
            inference_latency.as_millis(),
            audio_duration.as_millis()
        );
        Ok(AsrResult {
            text: res.text,
            inference_latency,
            audio_duration,
        })
    }
}

fn num_threads() -> i32 {
    // Use up to half the logical cores for inference; leave headroom for
    // the audio thread, the orchestrator, and any other userspace work.
    let n = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2);
    let half = (n / 2).max(1);
    i32::try_from(half).unwrap_or(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_err(r: Result<Asr, AsrError>) -> AsrError {
        match r {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        }
    }

    #[test]
    fn missing_model_file_surfaces_clear_error() {
        let paths = ModelPaths {
            model_int8_onnx: PathBuf::from("/nonexistent/model.int8.onnx"),
            tokens_txt: PathBuf::from("/nonexistent/tokens.txt"),
        };
        let cancel = CancellationToken::new();
        match must_err(Asr::new(&paths, &cancel)) {
            AsrError::ModelMissing(p) => assert!(p.ends_with("model.int8.onnx")),
            other => panic!("expected ModelMissing, got {other:?}"),
        }
    }

    #[test]
    fn pre_cancelled_token_short_circuits() {
        // model + tokens checks happen before cancellation, so the paths
        // must exist for the cancel-check to be reached. Use this crate's
        // own Cargo.toml as a stand-in; the recognizer init never actually
        // runs because cancel is checked first.
        let exists = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let paths = ModelPaths {
            model_int8_onnx: exists.clone(),
            tokens_txt: exists,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        match must_err(Asr::new(&paths, &cancel)) {
            AsrError::Cancelled => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }
}
