//! Concrete orchestrator wiring.

use std::sync::mpsc;
use std::time::Duration;

use medasr_asr::{spawn_worker, AsrCommand, AsrWorkerHandle, ModelPaths};
use medasr_audio::{
    capture::AudioCapture,
    resample::resample_to_16k_mono,
    EnergyVad, VadDecision, VadParams,
};
use medasr_focus::capture as capture_focus;
use medasr_inject::{EnigoBackend, FakeBackend, Injector, KeystrokeBackend};
use medasr_postprocess::{default_pipeline, Pipeline};
use medasr_secure_buffer::SecureBuffer;
use medasr_state::{AbortReason, ErrorClass, Event, Machine, State, TransitionEffect};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("audio: {0}")]
    Audio(String),
    #[error("asr: {0}")]
    Asr(String),
    #[error("inject: {0}")]
    Inject(String),
    #[error("focus: {0}")]
    Focus(String),
}

/// One-shot dictation cycle outcome — what happened, summarised.
#[derive(Debug)]
pub struct RunOnceOutcome {
    pub final_state: State,
    pub typed: Option<String>,
    pub abort_reason: Option<AbortReason>,
    pub error_class: Option<ErrorClass>,
}

/// The orchestrator holds the long-lived deps: the state machine, the ASR
/// worker handle, the post-process pipeline, and the injector. The cpal
/// `AudioCapture` is owned per-cycle (started on hotkey press, dropped on
/// release).
pub struct Orchestrator<B: KeystrokeBackend> {
    machine: Machine,
    asr: AsrWorkerHandle,
    pipeline: Pipeline,
    injector: Injector<B>,
    vad: EnergyVad,
}

impl Orchestrator<EnigoBackend> {
    /// Build with the production injector backend (enigo) and the default
    /// post-processing pipeline.
    pub fn new(model_paths: ModelPaths) -> Result<Self, OrchestratorError> {
        let asr = spawn_worker(model_paths)
            .map_err(|e| OrchestratorError::Asr(format!("{e:?}")))?;
        let injector = Injector::new(
            EnigoBackend::new().map_err(|e| OrchestratorError::Inject(format!("{e:?}")))?,
        );
        Ok(Self::with_parts(asr, injector, default_pipeline(), VadParams::default()))
    }
}

impl Orchestrator<FakeBackend> {
    /// Build with a fake injector — testing seam. The caller passes
    /// `model_paths` only because the ASR worker is real; if you need a
    /// fully-stubbed orchestrator (e.g. unit tests), build one directly
    /// with `with_parts` and a hand-rolled `AsrWorkerHandle` (not
    /// supported in v1; see future work in TESTING.md).
    pub fn with_fake_backend(
        model_paths: ModelPaths,
    ) -> Result<Self, OrchestratorError> {
        let asr = spawn_worker(model_paths)
            .map_err(|e| OrchestratorError::Asr(format!("{e:?}")))?;
        Ok(Self::with_parts(
            asr,
            Injector::new(FakeBackend::new()),
            default_pipeline(),
            VadParams::default(),
        ))
    }
}

impl<B: KeystrokeBackend> Orchestrator<B> {
    pub fn with_parts(
        asr: AsrWorkerHandle,
        injector: Injector<B>,
        pipeline: Pipeline,
        vad: VadParams,
    ) -> Self {
        let mut machine = Machine::new();
        // Drive directly into Ready for now; full onboarding flow lives
        // in src-tauri/UI. The personal/research CLI skips it.
        let _ = machine.on_event(Event::EulaAccepted); // Uninit -> EulaPending
        let _ = machine.on_event(Event::EulaAccepted); // EulaPending -> PermissionsPending
        let _ = machine.on_event(Event::PermissionsGranted);
        let _ = machine.on_event(Event::ModelDownloadStarted);
        let _ = machine.on_event(Event::ModelDownloadComplete);
        let _ = machine.on_event(Event::ModelVerifyOk);
        let _ = machine.on_event(Event::WarmupComplete);
        debug_assert_eq!(machine.state(), State::Ready);
        Self {
            machine,
            asr,
            pipeline,
            injector,
            vad: EnergyVad::new(vad),
        }
    }

    pub fn state(&self) -> State { self.machine.state() }

    /// Run a single press → release → typed-text cycle. Blocking.
    ///
    /// `record_for` is the maximum recording window. Real CLI uses a
    /// hotkey-release event to call this; this signature lets tests and
    /// the Phase 1A demo record-by-time.
    pub fn run_once(&mut self, record_for: Duration) -> RunOnceOutcome {
        // Auto-acknowledge any sticky soft-abort or error from the prior
        // cycle so the next press starts a fresh recording. Without this,
        // a NoSpeechDetected from the last cycle would jam the machine.
        if matches!(self.machine.state(), State::Aborted(_) | State::Error(_)) {
            self.machine.on_event(Event::Acknowledged);
        }

        let mut outcome = RunOnceOutcome {
            final_state: self.machine.state(),
            typed: None,
            abort_reason: None,
            error_class: None,
        };

        // 1. Capture focus + start cpal.
        let target = match capture_focus() {
            Ok(t) => t,
            Err(e) => {
                warn!("focus capture failed: {e}");
                self.machine.on_event(Event::Error(ErrorClass::TargetWindowLost));
                outcome.final_state = self.machine.state();
                outcome.error_class = Some(ErrorClass::TargetWindowLost);
                return outcome;
            }
        };
        info!("press: pid={} window={}", target.process_id, target.os_window_id);

        let effect = self.machine.on_event(Event::HotkeyPressed);
        if effect != TransitionEffect::StartRecording {
            warn!(?effect, state = ?self.machine.state(), "machine refused HotkeyPressed; skipping cycle");
            outcome.final_state = self.machine.state();
            return outcome;
        }

        let mut capture = match AudioCapture::start_default() {
            Ok(c) => c,
            Err(e) => {
                warn!("audio start failed: {e:?}");
                self.machine
                    .on_event(Event::Error(ErrorClass::PermissionDenied));
                outcome.final_state = self.machine.state();
                outcome.error_class = Some(ErrorClass::PermissionDenied);
                return outcome;
            }
        };

        // 2. Accumulate audio for `record_for`. Drain the SPSC ring
        // periodically into a scratch Vec<f32>; we resample at the end.
        let started = std::time::Instant::now();
        let mut raw: Vec<f32> = Vec::with_capacity(
            record_for.as_secs_f32() as usize
                * capture.config.sample_rate as usize
                * capture.config.channels as usize
                + 1024,
        );
        while started.elapsed() < record_for {
            while let Ok(s) = capture.consumer.pop() {
                raw.push(s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Final drain.
        while let Ok(s) = capture.consumer.pop() {
            raw.push(s);
        }
        let cfg = capture.config;
        drop(capture); // stops cpal stream

        // 3. Resample to 16 kHz mono i16.
        let resampled = match resample_to_16k_mono(&raw, cfg.sample_rate, cfg.channels) {
            Ok(s) => s,
            Err(e) => {
                warn!("resample failed: {e}");
                self.machine
                    .on_event(Event::Error(ErrorClass::InferenceFailed));
                outcome.final_state = self.machine.state();
                outcome.error_class = Some(ErrorClass::InferenceFailed);
                return outcome;
            }
        };

        // Debug aid: optionally dump the resampled audio to a wav so the
        // user can re-transcribe it via `medasr-cli wav` and isolate
        // capture from inference. Enabled by `MEDASR_DUMP_WAV=/path`.
        if let Ok(path) = std::env::var("MEDASR_DUMP_WAV") {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            match hound::WavWriter::create(&path, spec) {
                Ok(mut w) => {
                    for &s in &resampled {
                        let _ = w.write_sample(s);
                    }
                    let _ = w.finalize();
                    info!("dumped capture to {path}");
                }
                Err(e) => warn!("MEDASR_DUMP_WAV: {e}"),
            }
        }

        // 4. VAD gate.
        if matches!(self.vad.classify(&resampled), VadDecision::NoSpeech) {
            self.machine.on_event(Event::HotkeyReleased { held: record_for });
            self.machine.on_event(Event::NoSpeech);
            outcome.final_state = self.machine.state();
            outcome.abort_reason = Some(AbortReason::NoSpeechDetected);
            return outcome;
        }

        // 5. Wrap in SecureBuffer, hand off to ASR worker, await reply.
        let mut secure_buf = SecureBuffer::<i16>::with_capacity(resampled.len());
        secure_buf.as_mut_slice().copy_from_slice(&resampled);

        self.machine.on_event(Event::HotkeyReleased { held: record_for });

        let cancel = CancellationToken::new();
        let (reply_tx, reply_rx) = mpsc::channel();
        let send_result = self.asr.sender().send(AsrCommand::Transcribe {
            samples: secure_buf,
            cancel: cancel.clone(),
            reply: reply_tx,
        });
        if send_result.is_err() {
            self.machine.on_event(Event::Error(ErrorClass::InferenceFailed));
            outcome.final_state = self.machine.state();
            outcome.error_class = Some(ErrorClass::InferenceFailed);
            return outcome;
        }

        let asr_result = match reply_rx.recv() {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                warn!("asr error: {e:?}");
                self.machine.on_event(Event::Error(ErrorClass::InferenceFailed));
                outcome.final_state = self.machine.state();
                outcome.error_class = Some(ErrorClass::InferenceFailed);
                return outcome;
            }
            Err(_) => {
                self.machine.on_event(Event::Error(ErrorClass::InferenceFailed));
                outcome.final_state = self.machine.state();
                outcome.error_class = Some(ErrorClass::InferenceFailed);
                return outcome;
            }
        };

        self.machine.on_event(Event::TranscriptReady);
        info!(
            "asr raw: {:?} (inference {} ms over {} ms audio)",
            asr_result.text,
            asr_result.inference_latency.as_millis(),
            asr_result.audio_duration.as_millis()
        );

        // 6. Post-process.
        let typed = self.pipeline.run(&asr_result.text);

        // 7. Inject.
        if let Err(e) = self.injector.inject(&typed, &target) {
            warn!("inject failed: {e}");
            self.machine.on_event(Event::Error(ErrorClass::TargetWindowLost));
            outcome.final_state = self.machine.state();
            outcome.error_class = Some(ErrorClass::TargetWindowLost);
            return outcome;
        }

        self.machine.on_event(Event::InjectionComplete);
        outcome.final_state = self.machine.state();
        outcome.typed = Some(typed);
        outcome
    }
}

/// Convenience constructor used by the CLI/main: spin up an Orchestrator
/// from a model directory. Errors propagate out so the binary can
/// surface a clean message before exiting.
pub fn build(model_dir: &std::path::Path) -> Result<Orchestrator<EnigoBackend>, OrchestratorError> {
    let paths = ModelPaths::from_dir(model_dir);
    Orchestrator::new(paths)
}

/// Trait-equivalent interface for a generic "run one cycle" callable.
pub trait RunOnce {
    fn run_once(&mut self, record_for: Duration) -> RunOnceOutcome;
}

impl<B: KeystrokeBackend> RunOnce for Orchestrator<B> {
    fn run_once(&mut self, record_for: Duration) -> RunOnceOutcome {
        Orchestrator::run_once(self, record_for)
    }
}

// ---------------------------------------------------------------------
// Compatibility shim — keeping the orchestrator's audio entry point
// minimal even though `medasr-audio::capture::start` was the public name
// in Unit 2.
// ---------------------------------------------------------------------

trait StartDefault {
    fn start_default() -> Result<AudioCapture, OrchestratorError>;
}
impl StartDefault for AudioCapture {
    fn start_default() -> Result<AudioCapture, OrchestratorError> {
        medasr_audio::capture::start().map_err(|e| OrchestratorError::Audio(format!("{e:?}")))
    }
}

