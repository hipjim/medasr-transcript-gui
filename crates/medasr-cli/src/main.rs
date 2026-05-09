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

    medasr-cli dictate <model-dir>
        Run the full dictation cycle. Records 5s on hotkey press,
        transcribes via MedASR, types into the focused window.

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
        "help" | "--help" | "-h" => println!("{HELP}"),
        other => {
            eprintln!("unknown command: {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
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

fn run_dictate(model_dir: PathBuf) {
    info!("Phase 1A dictation. Press F12 to record 5 s. Ctrl-C to exit.");
    info!("model dir: {}", model_dir.display());

    let mut orch = match medasr_lifecycle::orchestrator_build(&model_dir) {
        Ok(o) => o,
        Err(e) => {
            error!("orchestrator init: {e}\n  Verify {} contains model.int8.onnx and tokens.txt.", model_dir.display());
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
