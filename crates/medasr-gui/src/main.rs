//! MedASR — native egui UI.
//!
//! Two surfaces:
//!
//!  1. **Live record** — click "Record 5 s", speak, the captured audio is
//!     transcribed and displayed in the transcript pane.
//!  2. **Open WAV** — pick a WAV file via the native file dialog and
//!     transcribe it. Useful for testing without a working mic, or for
//!     re-running the model on a previously-captured `MEDASR_DUMP_WAV`.
//!
//! Concurrency:
//! - GUI thread runs egui.
//! - cpal capture + ASR-worker handoff + post-processing runs on a
//!   per-job worker thread (cpal's `Stream` is `!Send` on macOS, so the
//!   capture must happen on whichever thread builds it).
//! - Worker sends the result back via mpsc; the GUI polls each frame.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};

use medasr_asr::{spawn_worker, AsrCommand, AsrWorkerHandle, ModelPaths};
use medasr_audio::{
    capture::{default_input_device, list_input_devices, start_with_device, InputDevice},
    high_pass_filter, peak_fraction,
    resample::resample_to_16k_mono,
    rms_dbfs, rms_normalize, spectral_subtract, trim_silence,
};
use medasr_postprocess::{default_pipeline, Pipeline};
use medasr_secure_buffer::SecureBuffer;
use tokio_util::sync::CancellationToken;

const RECORD_SECS: u64 = 5;
/// Hard cap for push-to-talk hold-time. Matches R10's 90 s recording cap.
const PTT_MAX_SECS: u64 = 90;

#[derive(Debug, Clone)]
enum Status {
    NoModel,
    EulaPending,
    Downloading {
        file: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Verifying,
    Idle,
    Recording { started: Instant },
    Transcribing,
    Error(String),
}

#[derive(Debug, Clone, Default)]
struct CycleStats {
    audio_ms: u128,
    inference_ms: u128,
    peak_pct: f32,
    rms_dbfs: f32,
    noise_dbfs: f32,
    snr_db: f32,
    /// Approximate speaking rate in words per minute, computed from the
    /// post-processed transcript over the trimmed audio length.
    wpm: f32,
    word_count: usize,
    /// Did spectral noise subtraction actually run on this clip?
    noise_subtracted: bool,
}

enum WorkerMsg {
    Transcript {
        raw: String,
        post: String,
        stats: CycleStats,
    },
    Error(String),
    /// First-run download progress.
    DlProgress { file: String, downloaded: u64, total: Option<u64> },
    DlVerifying,
    DlComplete(PathBuf),
    DlError(String),
}

struct App {
    status: Status,
    model_dir: Option<PathBuf>,
    transcript_raw: String,
    transcript_post: String,
    log: Vec<String>,
    pipeline: Pipeline,
    asr: Option<AsrWorkerHandle>,
    rx: Option<mpsc::Receiver<WorkerMsg>>,
    /// Push-to-talk: set to true while the user holds Space.
    ptt_active: bool,
    /// Cancellation signal for the active worker. Set to true on PTT
    /// release to stop the recording loop.
    ptt_stop: Option<Arc<AtomicBool>>,
    /// Available input devices and the chosen one. Refreshed on demand
    /// (cpal doesn't surface hot-plug events without polling).
    devices: Vec<InputDevice>,
    selected_device: Option<String>,
    /// Peak meter shared with the active capture's audio thread.
    live_peak: Option<Arc<std::sync::atomic::AtomicU32>>,
    last_peak_pct: f32,
    /// User toggles for the audio pipeline.
    enable_noise_subtraction: bool,
    /// Stats from the most recent transcribe.
    last_stats: Option<CycleStats>,
}

impl App {
    fn new() -> Self {
        Self {
            status: Status::NoModel,
            model_dir: None,
            transcript_raw: String::new(),
            transcript_post: String::new(),
            log: Vec::new(),
            pipeline: default_pipeline(),
            asr: None,
            rx: None,
            ptt_active: false,
            ptt_stop: None,
            devices: list_input_devices(),
            selected_device: default_input_device().map(|d| d.name),
            live_peak: None,
            last_peak_pct: 0.0,
            enable_noise_subtraction: true,
            last_stats: None,
        }
    }

    fn picked_device(&self) -> Option<InputDevice> {
        let name = self.selected_device.as_deref()?;
        self.devices.iter().find(|d| d.name == name).cloned()
    }

    fn log_line(&mut self, s: impl Into<String>) {
        let line = s.into();
        tracing::info!("{line}");
        self.log.push(line);
        if self.log.len() > 200 { self.log.drain(..self.log.len() - 200); }
    }

    fn load_model(&mut self, dir: PathBuf) {
        self.log_line(format!("loading model from {}", dir.display()));
        match spawn_worker(ModelPaths::from_dir(&dir)) {
            Ok(h) => {
                self.asr = Some(h);
                self.model_dir = Some(dir);
                self.status = Status::Idle;
                self.log_line("ASR worker ready");
            }
            Err(e) => {
                self.status = Status::Error(format!("model init: {e:?}"));
                self.log_line(format!("model init failed: {e:?}"));
            }
        }
    }

    fn start_record(&mut self) {
        let Some(asr) = self.asr.as_ref().map(|h| h.sender()) else {
            self.status = Status::Error("no model loaded".into());
            return;
        };
        let Some(device) = self.picked_device() else {
            self.status = Status::Error("no input device".into());
            return;
        };
        let pipeline_clone = self.pipeline_clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = Status::Recording { started: Instant::now() };
        self.log_line(format!("recording for {RECORD_SECS} s on {}...", device.name));
        let peak_share = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let peak_for_worker = Arc::clone(&peak_share);
        self.live_peak = Some(peak_share);
        let denoise = self.enable_noise_subtraction;

        thread::spawn(move || {
            let res = record_and_transcribe(
                asr,
                pipeline_clone,
                RecordSource::LiveTimed { secs: RECORD_SECS, device, peak: peak_for_worker },
                denoise,
            );
            match res {
                Ok(msg) => { let _ = tx.send(msg); }
                Err(e) => { let _ = tx.send(WorkerMsg::Error(format!("{e}"))); }
            }
        });
    }

    fn start_ptt(&mut self) -> Option<Arc<AtomicBool>> {
        let asr = self.asr.as_ref().map(|h| h.sender())?;
        let Some(device) = self.picked_device() else {
            self.status = Status::Error("no input device".into());
            return None;
        };
        let pipeline_clone = self.pipeline_clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = Status::Recording { started: Instant::now() };
        self.log_line(format!("push-to-talk: recording on {}...", device.name));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_worker = Arc::clone(&stop);
        let peak_share = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let peak_for_worker = Arc::clone(&peak_share);
        self.live_peak = Some(peak_share);
        let denoise = self.enable_noise_subtraction;
        thread::spawn(move || {
            let res = record_and_transcribe(
                asr,
                pipeline_clone,
                RecordSource::LivePtt {
                    stop: stop_for_worker,
                    max_secs: PTT_MAX_SECS,
                    device,
                    peak: peak_for_worker,
                },
                denoise,
            );
            match res {
                Ok(msg) => { let _ = tx.send(msg); }
                Err(e) => { let _ = tx.send(WorkerMsg::Error(format!("{e}"))); }
            }
        });
        Some(stop)
    }

    fn open_wav(&mut self, path: PathBuf) {
        let asr = match self.asr.as_ref() {
            Some(h) => h.sender(),
            None => {
                self.status = Status::Error("no model loaded".into());
                return;
            }
        };
        let pipeline_clone = self.pipeline_clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = Status::Transcribing;
        self.log_line(format!("transcribing {}", path.display()));
        let denoise = self.enable_noise_subtraction;

        thread::spawn(move || {
            let res = record_and_transcribe(
                asr,
                pipeline_clone,
                RecordSource::Wav(path),
                denoise,
            );
            match res {
                Ok(msg) => { let _ = tx.send(msg); }
                Err(e) => { let _ = tx.send(WorkerMsg::Error(format!("{e}"))); }
            }
        });
    }

    /// Pipeline isn't Clone; build a fresh default for each job.
    fn pipeline_clone(&self) -> Pipeline { default_pipeline() }

    fn poll_worker(&mut self) {
        let Some(rx) = self.rx.as_ref() else { return };
        loop {
            match rx.try_recv() {
                Ok(WorkerMsg::Transcript { raw, post, stats }) => {
                    self.transcript_raw = raw;
                    self.transcript_post = post;
                    self.log_line(format!(
                        "transcribed {} ms audio in {} ms (peak {:.0}% rms {:.1} dBFS snr {:.1} dB wpm {:.0})",
                        stats.audio_ms, stats.inference_ms, stats.peak_pct,
                        stats.rms_dbfs, stats.snr_db, stats.wpm,
                    ));
                    self.last_stats = Some(stats);
                    self.status = Status::Idle;
                    self.rx = None;
                    return;
                }
                Ok(WorkerMsg::Error(e)) => {
                    self.log_line(format!("worker error: {e}"));
                    self.status = Status::Error(e);
                    self.rx = None;
                    return;
                }
                Ok(WorkerMsg::DlProgress { file, downloaded, total }) => {
                    self.status = Status::Downloading { file, downloaded, total };
                    // keep draining
                }
                Ok(WorkerMsg::DlVerifying) => {
                    self.status = Status::Verifying;
                }
                Ok(WorkerMsg::DlComplete(dir)) => {
                    self.log_line(format!("model downloaded to {}", dir.display()));
                    self.rx = None;
                    self.load_model(dir);
                    return;
                }
                Ok(WorkerMsg::DlError(e)) => {
                    self.log_line(format!("download failed: {e}"));
                    self.status = Status::Error(format!("download: {e}"));
                    self.rx = None;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    if !matches!(self.status, Status::Error(_)) {
                        self.status = Status::Error("worker thread died".into());
                    }
                    return;
                }
            }
        }
    }

    fn start_download(&mut self) {
        let Some(cache_root) = medasr_paths::model_cache_dir().ok() else {
            self.status = Status::Error("could not resolve model cache dir".into());
            return;
        };
        let dest = cache_root.join(format!(
            "medasr-{}-{}",
            medasr_model::HF_REPO.replace('/', "_"),
            medasr_model::HF_REVISION,
        ));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = Status::Downloading {
            file: medasr_model::MANIFEST.first().map(|f| f.path.into()).unwrap_or_default(),
            downloaded: 0,
            total: None,
        };
        self.log_line("download: starting model fetch");
        thread::spawn(move || run_download(dest, tx));
    }

    fn accept_eula(&mut self) {
        match medasr_model::record_acceptance(&medasr_model::standard_record_path()) {
            Ok(_) => {
                self.log_line("EULA accepted");
                self.start_download();
            }
            Err(e) => {
                self.log_line(format!("could not record EULA acceptance: {e}"));
                self.status = Status::Error(format!("eula: {e}"));
            }
        }
    }

    fn decline_eula(&mut self) {
        self.log_line("EULA declined; user must accept to use MedASR");
        self.status = Status::NoModel;
    }
}

fn run_download(dest: PathBuf, tx: mpsc::Sender<WorkerMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(WorkerMsg::DlError(format!("tokio: {e}")));
            return;
        }
    };
    rt.block_on(async move {
        let client = medasr_model::default_client();
        let cancel = CancellationToken::new();
        for file in medasr_model::MANIFEST {
            let tx_progress = tx.clone();
            let path_for_msg = file.path.to_string();
            let res = medasr_model::fetch_file(
                &client,
                file,
                &dest,
                &cancel,
                move |p: medasr_model::Progress| {
                    let _ = tx_progress.send(WorkerMsg::DlProgress {
                        file: path_for_msg.clone(),
                        downloaded: p.downloaded,
                        total: p.total,
                    });
                },
            )
            .await;
            if let Err(e) = res {
                let _ = tx.send(WorkerMsg::DlError(format!("{e}")));
                return;
            }
        }
        let _ = tx.send(WorkerMsg::DlVerifying);
        for file in medasr_model::MANIFEST {
            let path = dest.join(file.path);
            if let Err(e) = medasr_model::verify_against_manifest(&path, file) {
                let _ = tx.send(WorkerMsg::DlError(format!("{e}")));
                return;
            }
        }
        let _ = tx.send(WorkerMsg::DlComplete(dest));
    });
}

enum RecordSource {
    /// Record for a fixed number of seconds.
    LiveTimed { secs: u64, device: InputDevice, peak: Arc<std::sync::atomic::AtomicU32> },
    /// Record until `stop` flips to true, capped at `max_secs`.
    LivePtt {
        stop: Arc<AtomicBool>,
        max_secs: u64,
        device: InputDevice,
        peak: Arc<std::sync::atomic::AtomicU32>,
    },
    Wav(PathBuf),
}

fn record_and_transcribe(
    asr_tx: mpsc::Sender<AsrCommand>,
    pipeline: Pipeline,
    source: RecordSource,
    enable_noise_subtraction: bool,
) -> Result<WorkerMsg, String> {
    let resampled: Vec<i16> = match source {
        RecordSource::LiveTimed { secs, device, peak } => {
            let mut capture = start_with_device(&device).map_err(|e| format!("audio start: {e:?}"))?;
            let cfg = capture.config;
            // Mirror cpal's peak-meter into the GUI's shared atomic.
            let peak_src = Arc::clone(&capture.peak_centi_pct);
            let started = Instant::now();
            let cap = Duration::from_secs(secs);
            let mut raw: Vec<f32> = Vec::with_capacity(
                secs as usize * cfg.sample_rate as usize * cfg.channels as usize + 1024,
            );
            while started.elapsed() < cap {
                while let Ok(s) = capture.consumer.pop() {
                    raw.push(s);
                }
                peak.store(peak_src.load(Ordering::Relaxed), Ordering::Relaxed);
                thread::sleep(Duration::from_millis(20));
            }
            while let Ok(s) = capture.consumer.pop() {
                raw.push(s);
            }
            peak.store(0, Ordering::Relaxed);
            drop(capture);
            resample_to_16k_mono(&raw, cfg.sample_rate, cfg.channels)
                .map_err(|e| format!("resample: {e}"))?
        }
        RecordSource::LivePtt { stop, max_secs, device, peak } => {
            let mut capture = start_with_device(&device).map_err(|e| format!("audio start: {e:?}"))?;
            let cfg = capture.config;
            let peak_src = Arc::clone(&capture.peak_centi_pct);
            let started = Instant::now();
            let cap = Duration::from_secs(max_secs);
            let mut raw: Vec<f32> = Vec::with_capacity(
                cfg.sample_rate as usize * cfg.channels as usize * 2,
            );
            while !stop.load(Ordering::Relaxed) && started.elapsed() < cap {
                while let Ok(s) = capture.consumer.pop() {
                    raw.push(s);
                }
                peak.store(peak_src.load(Ordering::Relaxed), Ordering::Relaxed);
                thread::sleep(Duration::from_millis(20));
            }
            while let Ok(s) = capture.consumer.pop() {
                raw.push(s);
            }
            peak.store(0, Ordering::Relaxed);
            drop(capture);
            resample_to_16k_mono(&raw, cfg.sample_rate, cfg.channels)
                .map_err(|e| format!("resample: {e}"))?
        }
        RecordSource::Wav(path) => {
            let mut reader = hound::WavReader::open(&path)
                .map_err(|e| format!("open wav: {e}"))?;
            let spec = reader.spec();
            let samples_native: Vec<f32> = match spec.sample_format {
                hound::SampleFormat::Int => reader
                    .samples::<i32>()
                    .filter_map(|s| s.ok())
                    .map(|s| {
                        let max = (1i32 << (spec.bits_per_sample - 1)) - 1;
                        s as f32 / max as f32
                    })
                    .collect(),
                hound::SampleFormat::Float => reader
                    .samples::<f32>()
                    .filter_map(|r| r.ok())
                    .collect(),
            };
            resample_to_16k_mono(&samples_native, spec.sample_rate, spec.channels)
                .map_err(|e| format!("resample: {e}"))?
        }
    };

    // 1. High-pass at 80 Hz to kill HVAC / fan / room rumble.
    let filtered = high_pass_filter(&resampled, 80.0);

    // 2. Optional spectral noise subtraction. Use the first 300 ms of
    //    the high-passed audio as the noise estimate. Skips if that
    //    head segment is signal-loud (which would mean the user spoke
    //    immediately on press).
    let noise_estimate_len = (16_000 * 3 / 10).min(filtered.len() / 2);
    let noise_segment = &filtered[..noise_estimate_len];
    let noise_dbfs_pre = rms_dbfs(noise_segment);
    let (denoised, noise_subtracted) = if enable_noise_subtraction {
        let out = spectral_subtract(&filtered, noise_segment, -25.0);
        // spectral_subtract returns input unchanged if it skipped.
        let actually_did = out != filtered;
        (out, actually_did)
    } else {
        (filtered, false)
    };

    // 3. Trim silence at head + tail.
    let trimmed = trim_silence(&denoised, 0.005, 400);
    let trimmed = if trimmed.is_empty() { denoised } else { trimmed };

    // 4. RMS-normalize to MedASR's training-distribution sweet spot.
    let leveled = rms_normalize(&trimmed, -20.0);

    let n = leveled.len();
    let audio_ms = (n as u128) * 1000 / 16_000;
    let peak_pct = peak_fraction(&leveled) * 100.0;
    let signal_dbfs = rms_dbfs(&leveled);
    let snr_db = signal_dbfs - noise_dbfs_pre;

    // Sherpa-onnx wraps an ONNX-runtime C++ implementation that throws
    // (and cannot be caught by Rust) when the input shape is too small
    // for the encoder's first conv kernel. Guard against PTT slips.
    const MIN_ASR_SAMPLES: usize = 16_000 / 4; // 250 ms
    if n < MIN_ASR_SAMPLES {
        return Ok(WorkerMsg::Transcript {
            raw: String::new(),
            post: String::new(),
            stats: CycleStats {
                audio_ms,
                peak_pct,
                rms_dbfs: signal_dbfs,
                noise_dbfs: noise_dbfs_pre,
                snr_db,
                noise_subtracted,
                ..Default::default()
            },
        });
    }

    let mut secure = SecureBuffer::<i16>::with_capacity(n);
    secure.as_mut_slice().copy_from_slice(&leveled);

    let cancel = CancellationToken::new();
    let (reply_tx, reply_rx) = mpsc::channel();
    asr_tx
        .send(AsrCommand::Transcribe { samples: secure, cancel, reply: reply_tx })
        .map_err(|_| "asr worker channel closed".to_string())?;

    let inference_started = Instant::now();
    let result = reply_rx
        .recv()
        .map_err(|_| "asr reply channel closed".to_string())?
        .map_err(|e| format!("asr error: {e:?}"))?;
    let inference_ms = inference_started.elapsed().as_millis();
    let post = pipeline.run(&result.text);
    let word_count = post.split_whitespace().count();
    let wpm = if audio_ms > 0 {
        (word_count as f32) / (audio_ms as f32 / 1000.0) * 60.0
    } else {
        0.0
    };
    Ok(WorkerMsg::Transcript {
        raw: result.text,
        post,
        stats: CycleStats {
            audio_ms,
            inference_ms,
            peak_pct,
            rms_dbfs: signal_dbfs,
            noise_dbfs: noise_dbfs_pre,
            snr_db,
            wpm,
            word_count,
            noise_subtracted,
        },
    })
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        // Repaint while a job is running so the timer / progress updates.
        if matches!(self.status, Status::Recording { .. } | Status::Transcribing) {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // ---- Push-to-talk: hold Space to record ----
        let space_down = ctx.input(|i| i.key_down(egui::Key::Space));
        if !space_down {
            // Released → stop the active PTT recording, if any.
            if self.ptt_active {
                if let Some(stop) = self.ptt_stop.take() {
                    stop.store(true, Ordering::Relaxed);
                    self.log_line("push-to-talk: stopping (release)");
                }
                self.ptt_active = false;
            }
        } else {
            // Pressed → start PTT if Idle.
            if !self.ptt_active && matches!(self.status, Status::Idle) {
                self.ptt_active = true;
                self.ptt_stop = self.start_ptt();
            }
        }

        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (label, color) = match &self.status {
                    Status::NoModel => ("⊘ no model", Color32::GRAY),
                    Status::EulaPending => ("⚠ eula pending", Color32::from_rgb(220, 160, 60)),
                    Status::Downloading { .. } => ("↓ downloading model", Color32::from_rgb(120, 180, 220)),
                    Status::Verifying => ("· verifying", Color32::from_rgb(120, 180, 220)),
                    Status::Idle => ("● ready", Color32::from_rgb(80, 180, 80)),
                    Status::Recording { started } => {
                        let elapsed = started.elapsed().as_secs_f32();
                        let remaining = (RECORD_SECS as f32) - elapsed;
                        let _ = remaining;
                        ("● recording", Color32::from_rgb(220, 80, 80))
                    }
                    Status::Transcribing => ("● transcribing", Color32::from_rgb(220, 160, 60)),
                    Status::Error(_) => ("✕ error", Color32::from_rgb(220, 80, 80)),
                };
                ui.label(RichText::new(label).color(color).strong());
                ui.separator();
                if let Status::Recording { started } = &self.status {
                    let elapsed = started.elapsed().as_secs_f32();
                    let remaining = ((RECORD_SECS as f32) - elapsed).max(0.0);
                    ui.label(format!("{remaining:.1} s remaining"));
                }
                if let Status::Error(e) = &self.status {
                    ui.label(RichText::new(e).color(Color32::from_rgb(220, 80, 80)));
                }
                if let Some(d) = &self.model_dir {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(d.display().to_string()).weak());
                    });
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // First-run flow: NoModel -> show options.
            if matches!(self.status, Status::NoModel) {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.heading("MedASR");
                    ui.label(
                        RichText::new("Local-only radiology dictation. Choose how to set up:")
                            .weak(),
                    );
                    ui.add_space(14.0);
                    if ui.add(egui::Button::new("⬇  Get the model (≈ 150 MB from Hugging Face)").min_size(egui::vec2(360.0, 36.0))).clicked() {
                        self.status = Status::EulaPending;
                    }
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new("📁  I already have the model — pick a folder…").min_size(egui::vec2(360.0, 36.0))).clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.load_model(dir);
                        }
                    }
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new("The folder must contain model.int8.onnx and tokens.txt.")
                            .weak()
                            .small(),
                    );
                });
                return;
            }

            // EULA gate.
            if matches!(self.status, Status::EulaPending) {
                ui.heading("Health AI Developer Foundations Terms");
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - 80.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(medasr_model::eula::EULA_TEXT).monospace());
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("✓ Accept & download").min_size(egui::vec2(180.0, 32.0))).clicked() {
                        self.accept_eula();
                    }
                    if ui.button("✕ Decline").clicked() {
                        self.decline_eula();
                    }
                });
                return;
            }

            // Download progress.
            if let Status::Downloading { file, downloaded, total } = self.status.clone() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.heading("Downloading MedASR model");
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("File: {file}")).weak());
                    ui.add_space(8.0);
                    let mb = downloaded as f64 / 1_048_576.0;
                    let frac = match total {
                        Some(t) if t > 0 => downloaded as f32 / t as f32,
                        _ => 0.0,
                    };
                    let label = match total {
                        Some(t) => format!("{:.1} / {:.1} MB", mb, t as f64 / 1_048_576.0),
                        None => format!("{mb:.1} MB"),
                    };
                    ui.add(egui::ProgressBar::new(frac).desired_width(360.0).text(label));
                    ui.add_space(20.0);
                    ui.label(RichText::new("Downloads to your OS cache directory.").weak().small());
                });
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }

            if matches!(self.status, Status::Verifying) {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.heading("Verifying download");
                    ui.add_space(8.0);
                    ui.label(RichText::new("Checking SHA-256 against the bundled manifest…").weak());
                    ui.add_space(20.0);
                    ui.spinner();
                });
                ctx.request_repaint_after(Duration::from_millis(200));
                return;
            }

            // Device picker row.
            let busy = matches!(self.status, Status::Recording { .. } | Status::Transcribing);
            ui.horizontal(|ui| {
                ui.label("Input:");
                let current = self.selected_device.clone().unwrap_or_else(|| "<none>".into());
                let mut new_selection: Option<String> = None;
                let combo = egui::ComboBox::from_id_salt("input-device").selected_text(current.clone()).width(280.0);
                combo.show_ui(ui, |ui| {
                    for d in &self.devices {
                        if ui.selectable_label(self.selected_device.as_deref() == Some(&d.name), &d.name).clicked() {
                            new_selection = Some(d.name.clone());
                        }
                    }
                });
                if let Some(n) = new_selection {
                    self.selected_device = Some(n);
                }
                if ui.button("Refresh").clicked() {
                    self.devices = list_input_devices();
                    self.log_line(format!("refreshed {} input devices", self.devices.len()));
                }
            });

            // Live peak meter (visible while recording).
            let peak = self
                .live_peak
                .as_ref()
                .map(|p| p.load(Ordering::Relaxed) as f32 / 10_000.0)
                .unwrap_or(0.0);
            if matches!(self.status, Status::Recording { .. }) {
                self.last_peak_pct = (self.last_peak_pct * 0.6 + peak * 0.4).max(peak);
            } else {
                self.last_peak_pct = (self.last_peak_pct * 0.6 + peak * 0.4).max(0.0);
            }
            let bar_color = if self.last_peak_pct < 0.05 { Color32::from_rgb(120, 120, 120) }
                else if self.last_peak_pct < 0.5 { Color32::from_rgb(80, 180, 80) }
                else if self.last_peak_pct < 0.85 { Color32::from_rgb(220, 200, 60) }
                else { Color32::from_rgb(220, 80, 80) };
            ui.horizontal(|ui| {
                ui.label("Mic:");
                let bar = egui::ProgressBar::new(self.last_peak_pct)
                    .desired_width(280.0)
                    .fill(bar_color)
                    .text(format!("{:.0}% ({:.0} dBFS)", self.last_peak_pct * 100.0, dbfs_from(self.last_peak_pct)));
                ui.add(bar);
            });

            ui.add_space(4.0);

            // Action row.
            ui.horizontal(|ui| {
                let record_label = format!("🎤 Record {RECORD_SECS} s");
                if ui.add_enabled(!busy, egui::Button::new(record_label).min_size(egui::vec2(150.0, 32.0))).clicked() {
                    self.start_record();
                }
                if ui.add_enabled(!busy, egui::Button::new("📁 Open WAV…").min_size(egui::vec2(120.0, 32.0))).clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("wav", &["wav"]).pick_file() {
                        self.open_wav(path);
                    }
                }
                if ui.add_enabled(!busy, egui::Button::new("Clear").min_size(egui::vec2(60.0, 32.0))).clicked() {
                    self.transcript_raw.clear();
                    self.transcript_post.clear();
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Tip: hold SPACE to push-to-talk (release to transcribe).")
                        .weak()
                        .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.enable_noise_subtraction, "Noise subtraction");
                });
            });
            ui.add_space(4.0);
            ui.separator();

            // Per-recording stats panel.
            if let Some(s) = self.last_stats.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Last:").strong());
                    stat_chip(ui, "audio", &format!("{} ms", s.audio_ms));
                    stat_chip(ui, "infer", &format!("{} ms", s.inference_ms));
                    stat_chip(ui, "peak", &format!("{:.0}%", s.peak_pct));
                    stat_chip(ui, "rms", &format!("{:.1} dBFS", s.rms_dbfs));
                    stat_chip(ui, "noise", &format!("{:.1} dBFS", s.noise_dbfs));
                    stat_chip(ui, "snr", &format!("{:.1} dB", s.snr_db));
                    stat_chip(ui, "wpm", &format!("{:.0} ({} words)", s.wpm, s.word_count));
                    if s.noise_subtracted {
                        ui.label(RichText::new("• denoised").color(Color32::from_rgb(120, 180, 220)).small());
                    }
                });
                ui.add_space(4.0);
            }

            // Transcript pane.
            ui.label(RichText::new("Transcript").strong());
            ui.add_space(4.0);
            let avail = ui.available_size();
            egui::ScrollArea::vertical()
                .max_height(avail.y - 220.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.transcript_post.as_str())
                            .desired_width(f32::INFINITY)
                            .desired_rows(8)
                            .font(egui::TextStyle::Monospace),
                    );
                });

            ui.add_space(8.0);
            ui.collapsing("Raw model output", |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.transcript_raw.as_str())
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .font(egui::TextStyle::Monospace),
                );
            });

            ui.add_space(4.0);
            ui.collapsing(format!("Log ({})", self.log.len()), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(RichText::new(line).font(egui::FontId::monospace(11.0)));
                        }
                    });
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    let mut app = App::new();
    // Try a few well-known locations for the model so a returning user
    // doesn't have to walk the first-run flow again:
    //   1. ~/medasr-model (dev convenience).
    //   2. The OS cache dir we wrote to during a previous first-run flow.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs_home() {
        candidates.push(home.join("medasr-model"));
    }
    if let Ok(cache_root) = medasr_paths::model_cache_dir() {
        candidates.push(cache_root.join(format!(
            "medasr-{}-{}",
            medasr_model::HF_REPO.replace('/', "_"),
            medasr_model::HF_REVISION,
        )));
    }
    for dir in candidates {
        if dir.join("model.int8.onnx").is_file() && dir.join("tokens.txt").is_file() {
            app.load_model(dir);
            break;
        }
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 540.0])
        .with_min_inner_size([520.0, 380.0])
        .with_title("MedASR");
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("MedASR", opts, Box::new(|_cc| Ok(Box::new(app))))
}

/// Load the embedded PNG icon and convert it to the egui `IconData`
/// representation. Returns `None` on any decode failure rather than
/// crashing — the app still runs, the OS just falls back to a default.
fn load_window_icon() -> Option<std::sync::Arc<egui::IconData>> {
    const PNG: &[u8] = include_bytes!("../../../assets/icon/icon-256.png");
    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(std::sync::Arc::new(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    }))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn dbfs_from(linear: f32) -> f32 {
    if linear <= f32::EPSILON { -100.0 } else { 20.0 * linear.log10() }
}

fn stat_chip(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(
        RichText::new(format!("{label}: {value}"))
            .small()
            .background_color(Color32::from_rgb(38, 42, 50))
            .color(Color32::from_rgb(220, 220, 220)),
    );
}
