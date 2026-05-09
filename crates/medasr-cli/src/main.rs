//! Phase 1 vertical-slice demo.
//!
//! Loop:
//!   1. Register the global hotkey (F12 by default).
//!   2. Wait for press.
//!   3. Capture two-factor focus pin via `medasr-focus`.
//!   4. Wait 200 ms (the plan's "click in the report" buffer).
//!   5. Inject the literal string "MEDASR PLACEHOLDER" via the
//!      `KeystrokeBackend` chosen by `FocusTarget::chunking_policy`.
//!
//! The audio + ASR + post-process path is wired in Phase 2 (Unit 5).
//!
//! macOS specifically: this needs Accessibility permission for keystroke
//! synthesis and Input Monitoring permission for the global hotkey.
//! First run will be denied; grant in System Settings → Privacy & Security
//! and relaunch.

use std::time::Duration;

use medasr_focus::capture as capture_focus;
use medasr_hotkey::{default_hotkey, HotkeyService};
use medasr_inject::{EnigoBackend, Injector};
use tracing::{error, info};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("info".parse().unwrap()))
        .init();

    info!("MedASR vertical-slice demo (Phase 1B placeholder loop)");
    info!("Press F12 to inject 'MEDASR PLACEHOLDER' into the focused window.");
    info!("Ctrl-C to exit.");

    let hk = match HotkeyService::register(default_hotkey()) {
        Ok(s) => s,
        Err(e) => {
            error!(
                "could not register hotkey: {e}\n  \
                On macOS, grant Input Monitoring permission to your terminal.\n  \
                On Linux, ensure the X11 session is active (Wayland is unsupported)."
            );
            std::process::exit(1);
        }
    };

    let mut injector = match EnigoBackend::new() {
        Ok(b) => Injector::new(b),
        Err(e) => {
            error!(
                "could not initialise injection backend: {e}\n  \
                On macOS, grant Accessibility permission to your terminal."
            );
            std::process::exit(1);
        }
    };

    while hk.recv().is_some() {
        let target = match capture_focus() {
            Ok(t) => t,
            Err(e) => {
                error!("focus capture failed: {e}");
                continue;
            }
        };
        info!(
            "focus pin: pid={} window={} policy={:?}",
            target.process_id, target.os_window_id, target.chunking_policy
        );

        // The 200 ms gap matches the plan's vertical-slice spec.
        std::thread::sleep(Duration::from_millis(200));

        // Revalidate focus before typing — defends against "user clicked
        // away between press and timer fire".
        if let Err(e) = medasr_focus::revalidate(&target) {
            error!("focus pin lost between press and inject: {e}");
            continue;
        }

        if let Err(e) = injector.inject("MEDASR PLACEHOLDER", &target) {
            error!("inject failed: {e}");
            continue;
        }
        info!("injected");
    }
}
