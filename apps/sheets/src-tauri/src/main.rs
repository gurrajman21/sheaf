// Sheaf desktop app: a window for the React UI, with every UI command routed
// to the same backend (`sheets_core::Session::dispatch`) the dev bridge uses.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use serde_json::Value;
use sheets_core::Session;
use tauri::{Manager, State};

#[tauri::command]
async fn api(state: State<'_, Mutex<Session>>, cmd: String, args: Value) -> Result<Value, String> {
    let mut session = state.lock().map_err(|_| "the workbook is busy".to_string())?;
    let result = session.dispatch(&cmd, &args);
    #[cfg(debug_assertions)]
    eprintln!("[api] {cmd} -> {}", if result.is_ok() { "ok".to_string() } else { format!("{result:?}") });
    result
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let mut session = Session::new();
            // `sheaf path/to/file.xlsx` opens that file
            if let Some(path) = std::env::args().skip(1).find(|a| a.to_lowercase().ends_with(".xlsx") || a.to_lowercase().ends_with(".xlsm")) {
                if let Err(e) = session.open(std::path::Path::new(&path)) {
                    eprintln!("could not open {path}: {e}");
                }
            }
            app.manage(Mutex::new(session));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api])
        .run(tauri::generate_context!())
        .expect("error while running Sheaf");
}
