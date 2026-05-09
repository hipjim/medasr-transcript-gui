//! Tray-icon scaffold. Full state-aware tray (idle/recording/transcribing/
//! error) lands in Unit 7 using the `tauri::tray` core module
//! (`TrayIconBuilder`, `TrayIconEvent`).

use tauri::{App, tray::TrayIconBuilder};

pub fn install(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let _tray = TrayIconBuilder::new()
        .tooltip("MedASR")
        .build(app.handle())?;
    Ok(())
}
