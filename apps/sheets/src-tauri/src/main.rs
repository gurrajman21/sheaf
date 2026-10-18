// Sheaf desktop app: a window for the React UI, with every UI command routed
// to the same backend (`sheets_core::Session::dispatch`) the dev bridge uses,
// and a native macOS menu bar whose items are forwarded to the UI as actions.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use serde_json::Value;
use sheets_core::Session;
use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State, Wry};

#[tauri::command]
async fn api(state: State<'_, Mutex<Session>>, cmd: String, args: Value) -> Result<Value, String> {
    let mut session = state.lock().map_err(|_| "the workbook is busy".to_string())?;
    let result = session.dispatch(&cmd, &args);
    #[cfg(debug_assertions)]
    eprintln!("[api] {cmd} -> {}", if result.is_ok() { "ok".to_string() } else { format!("{result:?}") });
    result
}

/// Open the system print dialog for the window (it offers "Save as PDF").
#[tauri::command]
fn print_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.print().map_err(|e| e.to_string())
}

/// A menu item whose id is the UI action it triggers.
fn item(app: &AppHandle, id: &str, text: &str, accel: Option<&str>) -> tauri::Result<tauri::menu::MenuItem<Wry>> {
    let b = MenuItemBuilder::with_id(id, text);
    match accel {
        Some(a) => b.accelerator(a).build(app),
        None => b.build(app),
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let sep = || PredefinedMenuItem::separator(app);
    let app_menu = SubmenuBuilder::new(app, "Sheaf")
        .item(&PredefinedMenuItem::about(app, Some("About Sheaf"), Some(AboutMetadata {
            comments: Some("A spreadsheet editor that keeps Excel files intact.".into()),
            ..Default::default()
        }))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::services(app, None)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::hide(app, None)?)
        .item(&PredefinedMenuItem::hide_others(app, None)?)
        .item(&PredefinedMenuItem::show_all(app, None)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::quit(app, None)?)
        .build()?;
    let file: Submenu<Wry> = SubmenuBuilder::new(app, "File")
        .item(&item(app, "file:new", "New Workbook", Some("CmdOrCtrl+N"))?)
        .item(&item(app, "file:start", "New from Template…", Some("CmdOrCtrl+Shift+N"))?)
        .item(&item(app, "file:open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&sep()?)
        .item(&item(app, "file:save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item(app, "file:save_as", "Save As…", Some("CmdOrCtrl+Shift+S"))?)
        .item(&sep()?)
        .item(&item(app, "file:print", "Print or Save as PDF…", Some("CmdOrCtrl+P"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&item(app, "edit:undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item(app, "edit:redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .item(&sep()?)
        // standard items reach text fields too; the grid handles them as copy/cut/paste events
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&item(app, "edit:paste_values", "Paste Values", Some("CmdOrCtrl+Shift+V"))?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .item(&sep()?)
        .item(&item(app, "edit:fill_down", "Fill Down", Some("CmdOrCtrl+D"))?)
        .item(&item(app, "edit:fill_right", "Fill Right", Some("CmdOrCtrl+R"))?)
        .item(&item(app, "edit:clear", "Clear Contents", None)?)
        .item(&sep()?)
        .item(&item(app, "edit:find", "Find…", Some("CmdOrCtrl+F"))?)
        .item(&item(app, "edit:replace", "Replace…", Some("CmdOrCtrl+H"))?)
        .item(&item(app, "edit:goto", "Go To…", Some("CmdOrCtrl+G"))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item(app, "view:palette", "Command Palette…", Some("CmdOrCtrl+K"))?)
        .item(&sep()?)
        .item(&item(app, "view:zoom_in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item(app, "view:zoom_out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item(app, "view:zoom_reset", "Actual Size", Some("CmdOrCtrl+0"))?)
        .item(&sep()?)
        .item(&item(app, "view:gridlines", "Gridlines", None)?)
        .item(&item(app, "view:formulas", "Show Formulas", Some("Ctrl+`"))?)
        .item(&item(app, "view:freeze", "Freeze Panes at Selection", None)?)
        .item(&item(app, "view:unfreeze", "Unfreeze Panes", None)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;
    let insert = SubmenuBuilder::new(app, "Insert")
        .item(&item(app, "insert:rows", "Rows Above", None)?)
        .item(&item(app, "insert:cols", "Columns Left", None)?)
        .item(&item(app, "insert:sheet", "Sheet", Some("Shift+F11"))?)
        .item(&sep()?)
        .item(&item(app, "insert:chart", "Chart", Some("Alt+F1"))?)
        .item(&item(app, "insert:function", "Function…", Some("Shift+F3"))?)
        .item(&item(app, "insert:sum", "AutoSum", Some("CmdOrCtrl+Shift+T"))?)
        .item(&sep()?)
        .item(&item(app, "insert:date", "Today's Date", Some("CmdOrCtrl+;"))?)
        .build()?;
    let format = SubmenuBuilder::new(app, "Format")
        .item(&item(app, "format:bold", "Bold", Some("CmdOrCtrl+B"))?)
        .item(&item(app, "format:italic", "Italic", Some("CmdOrCtrl+I"))?)
        .item(&item(app, "format:underline", "Underline", Some("CmdOrCtrl+U"))?)
        .item(&sep()?)
        .item(&item(app, "format:painter", "Format Painter", None)?)
        .item(&item(app, "format:clear", "Clear Formatting", None)?)
        .item(&item(app, "format:merge", "Merge & Center", None)?)
        .item(&item(app, "format:autofit", "Auto-fit Column Width", None)?)
        .build()?;
    let data = SubmenuBuilder::new(app, "Data")
        .item(&item(app, "data:sort_asc", "Sort A to Z", None)?)
        .item(&item(app, "data:sort_desc", "Sort Z to A", None)?)
        .item(&item(app, "data:filter", "Filter", Some("CmdOrCtrl+Shift+L"))?)
        .item(&sep()?)
        .item(&item(app, "data:dedupe", "Remove Duplicates", None)?)
        .item(&item(app, "data:split", "Text to Columns", None)?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, None)?)
        .build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &insert, &format, &data, &window])
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
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            // the UI owns every action; standard items (cut/copy/paste...) never get here
            let _ = app.emit("menu", event.id().0.clone());
        })
        .invoke_handler(tauri::generate_handler![api, print_window])
        .run(tauri::generate_context!())
        .expect("error while running Sheaf");
}
