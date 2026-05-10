//! MedASR CLI.
//!
//! Two modes:
//!
//! - `medasr-cli placeholder` — Phase 1B vertical-slice loop. Hotkey-press
//!   captures focus and types the literal "MEDASR PLACEHOLDER". No audio,
//!   no ASR. Useful for verifying the integration spine on a target host
//!   without a model.
//!
//! - `medasr-cli dictate <model-dir>` — full dictation cycle. Records for
//!   5 seconds, transcribes with MedASR, post-processes (commands +
//!   numbers + caps), then types into the focused window. Requires the
//!   model files under `<model-dir>/{model.int8.onnx,tokens.txt}`.
//!
//! macOS: needs Accessibility permission for keystroke synthesis and
//! Input Monitoring permission for the global hotkey.

use std::path::PathBuf;
use std::time::Duration;

use medasr_focus::capture as capture_focus;
use medasr_hotkey::{default_hotkey, HotkeyService};
use medasr_inject::{EnigoBackend, Injector};
use tracing::{error, info};

const HELP: &str = "\
MedASR CLI

USAGE:
    medasr-cli placeholder
        Run the Phase 1B vertical-slice loop (no audio / no ASR).

    medasr-cli placeholder-once [delay-secs]
        Wait `delay-secs` (default 5), capture focus, inject 'MEDASR
        PLACEHOLDER' once, exit. Useful as a smoke test when the
        global-hotkey permission can't be granted to the debug binary.

    medasr-cli dictate <model-dir>
        Run the full dictation cycle. Records 5s on hotkey press,
        transcribes via MedASR, types into the focused window.

    medasr-cli wav <model-dir> <wav-file>
        Transcribe a WAV file directly. Bypasses cpal and the keystroke
        injector — prints the transcript to stdout. Useful for isolating
        the ASR wiring from the audio-capture pipeline.

    medasr-cli mics
        List available audio input devices and the default one cpal
        selects. Useful when the live capture comes back silent.

    medasr-cli help
        Print this message.
";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
    match cmd {
        "placeholder" => run_placeholder(),
        "placeholder-once" => {
            let delay = args.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(5);
            run_placeholder_once(Duration::from_secs(delay));
        }
        "dictate" => {
            let model_dir = match args.get(2) {
                Some(p) => PathBuf::from(p),
                None => {
                    eprintln!("error: dictate requires a <model-dir> arg.\n\n{HELP}");
                    std::process::exit(2);
                }
            };
            run_dictate(model_dir);
        }
        "mics" => run_mics(),
        "wav" => {
            let model_dir = match args.get(2) {
                Some(p) => PathBuf::from(p),
                None => {
                    eprintln!("error: wav requires <model-dir> <wav-file>.\n\n{HELP}");
                    std::process::exit(2);
                }
            };
            let wav_file = match args.get(3) {
                Some(p) => PathBuf::from(p),
                None => {
                    eprintln!("error: wav requires <wav-file>.\n\n{HELP}");
                    std::process::exit(2);
                }
            };
            run_wav(model_dir, wav_file);
        }
        "help" | "--help" | "-h" => println!("{HELP}"),
        other => {
            eprintln!("unknown command: {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn run_placeholder_once(delay: Duration) {
    info!(
        "placeholder-once: focus a text editor; injecting 'MEDASR PLACEHOLDER' in {}s",
        delay.as_secs()
    );
    std::thread::sleep(delay);
    let target = match capture_focus() {
        Ok(t) => t,
        Err(e) => {
            error!("focus capture: {e}");
            std::process::exit(1);
        }
    };
    info!(
        "focus pin: pid={} window={} policy={:?}",
        target.process_id, target.os_window_id, target.chunking_policy
    );
    let mut injector = match EnigoBackend::new() {
        Ok(b) => Injector::new(b),
        Err(e) => {
            error!("inject init: {e}\n  On macOS grant Accessibility to your terminal.");
            std::process::exit(1);
        }
    };
    if let Err(e) = injector.inject("MEDASR PLACEHOLDER", &target) {
        error!("inject: {e}");
        std::process::exit(1);
    }
    info!("injected");
}

fn run_placeholder() {
    info!("Phase 1B placeholder loop. Press F12 to inject 'MEDASR PLACEHOLDER'. Ctrl-C to exit.");
    let hk = match HotkeyService::register(default_hotkey()) {
        Ok(s) => s,
        Err(e) => {
            error!("hotkey register: {e}\n  On macOS grant Input Monitoring to your terminal.");
            std::process::exit(1);
        }
    };
    let mut injector = match EnigoBackend::new() {
        Ok(b) => Injector::new(b),
        Err(e) => {
            error!("inject init: {e}\n  On macOS grant Accessibility to your terminal.");
            std::process::exit(1);
        }
    };
    while hk.recv().is_some() {
        let target = match capture_focus() {
            Ok(t) => t,
            Err(e) => {
                error!("focus capture: {e}");
                continue;
            }
        };
        std::thread::sleep(Duration::from_millis(200));
        if medasr_focus::revalidate(&target).is_err() {
            error!("focus pin lost between press and inject");
            continue;
        }
        if let Err(e) = injector.inject("MEDASR PLACEHOLDER", &target) {
            error!("inject: {e}");
            continue;
        }
        info!("injected");
    }
}

fn run_mics() {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    println!("host: {}", host.id().name());
    match host.default_input_device() {
        Some(d) => println!(
            "default input: {}",
            d.name().unwrap_or_else(|_| "<unnamed>".into())
        ),
        None => println!("default input: <none>"),
    }
    println!();
    println!("available input devices:");
    match host.input_devices() {
        Ok(it) => {
            for d in it {
                let name = d.name().unwrap_or_else(|_| "<unnamed>".into());
                let cfg = d
                    .default_input_config()
                    .map(|c| {
                        format!(
                            "{} Hz × {} ch ({:?})",
                            c.sample_rate().0,
                            c.channels(),
                            c.sample_format()
                        )
                    })
                    .unwrap_or_else(|e| format!("(no config: {e})"));
                println!("  - {name:<40} {cfg}");
            }
        }
        Err(e) => println!("error listing input devices: {e}"),
    }
}

fn run_wav(model_dir: PathBuf, wav_file: PathBuf) {
    use medasr_asr::{spawn_worker, AsrCommand, ModelPaths};
    use medasr_secure_buffer::SecureBuffer;
    use std::sync::mpsc;

    info!(
        "wav-mode: model={} wav={}",
        model_dir.display(),
        wav_file.display()
    );
    let mut reader = match hound::WavReader::open(&wav_file) {
        Ok(r) => r,
        Err(e) => {
            error!("open wav: {e}");
            std::process::exit(1);
        }
    };
    let spec = reader.spec();
    info!("wav spec: {:?}", spec);

    // Convert any wav to 16k mono i16.
    let samples_native: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| {
                let max = (1i32 << (spec.bits_per_sample - 1)) - 1;
                s as f32 / max as f32
            })
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|r| r.ok()).collect(),
    };
    let samples_16k_i16 = match medasr_audio::resample_to_16k_mono(
        &samples_native,
        spec.sample_rate,
        spec.channels,
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("resample: {e}");
            std::process::exit(1);
        }
    };
    info!(
        "samples after resample: {} ({} ms @ 16 kHz)",
        samples_16k_i16.len(),
        samples_16k_i16.len() / 16
    );

    let asr = match spawn_worker(ModelPaths::from_dir(&model_dir)) {
        Ok(a) => a,
        Err(e) => {
            error!("asr init: {e:?}");
            std::process::exit(1);
        }
    };
    let mut secure = SecureBuffer::<i16>::with_capacity(samples_16k_i16.len());
    secure.as_mut_slice().copy_from_slice(&samples_16k_i16);

    let cancel = tokio_util::sync::CancellationToken::new();
    let (tx, rx) = mpsc::channel();
    asr.sender()
        .send(AsrCommand::Transcribe {
            samples: secure,
            cancel,
            reply: tx,
        })
        .unwrap();
    match rx.recv().unwrap() {
        Ok(r) => {
            println!("transcript: {:?}", r.text);
            println!("inference: {:?}", r.inference_latency);
            println!("audio: {:?}", r.audio_duration);
        }
        Err(e) => {
            eprintln!("asr error: {e:?}");
            std::process::exit(1);
        }
    }
}

fn run_dictate(model_dir: PathBuf) {
    info!("Phase 1A dictation. Press F12 to record 5 s. Ctrl-C to exit.");
    info!("model dir: {}", model_dir.display());

    let mut orch = match medasr_lifecycle::orchestrator_build(&model_dir) {
        Ok(o) => o,
        Err(e) => {
            error!(
                "orchestrator init: {e}\n  Verify {} contains model.int8.onnx and tokens.txt.",
                model_dir.display()
            );
            std::process::exit(1);
        }
    };

    let hk = match HotkeyService::register(default_hotkey()) {
        Ok(s) => s,
        Err(e) => {
            error!("hotkey register: {e}\n  On macOS grant Input Monitoring to your terminal.");
            std::process::exit(1);
        }
    };

    while hk.recv().is_some() {
        let outcome = orch.run_once(Duration::from_secs(5));
        if let Some(t) = &outcome.typed {
            info!("dictation typed: {t:?}");
        } else if let Some(r) = outcome.abort_reason {
            info!("aborted: {r:?}");
        } else if let Some(e) = outcome.error_class {
            error!("error: {e:?}");
        }
    }

    let _ = orch; // dropped here -> ASR worker shut down
}
