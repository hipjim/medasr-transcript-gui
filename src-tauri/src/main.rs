// Prevent additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tray;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            tray::install(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running medasr-app");
}
