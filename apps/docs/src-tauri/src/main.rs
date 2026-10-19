// Quire desktop app: a window for the React UI, with every UI command routed
// to the same backend (`docs_core::Session::dispatch`) the dev bridge uses,
// and a native macOS menu bar whose items are forwarded to the UI as actions.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

docs_core::fast_oxi_env!();

use std::sync::Mutex;

use docs_core::Session;
use serde_json::Value;
use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State, Wry};

#[tauri::command]
async fn api(state: State<'_, Mutex<Session>>, cmd: String, args: Value) -> Result<Value, String> {
    let mut session = state.lock().map_err(|_| "the document is busy".to_string())?;
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
    let app_menu = SubmenuBuilder::new(app, "Quire")
        .item(&PredefinedMenuItem::about(app, Some("About Quire"), Some(AboutMetadata {
            comments: Some("A word processor that keeps Word files intact.".into()),
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
        .item(&item(app, "file:new", "New Document", Some("CmdOrCtrl+N"))?)
        .item(&item(app, "file:start", "New from Template…", Some("CmdOrCtrl+Shift+N"))?)
        .item(&item(app, "file:open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&sep()?)
        .item(&item(app, "file:save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item(app, "file:save_as", "Save As…", Some("CmdOrCtrl+Shift+S"))?)
        .item(&item(app, "file:versions", "Version History…", None)?)
        .item(&sep()?)
        .item(&item(app, "file:print", "Print or Save as PDF…", Some("CmdOrCtrl+P"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&item(app, "edit:undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item(app, "edit:redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&item(app, "edit:select_all", "Select All", Some("CmdOrCtrl+A"))?)
        .item(&sep()?)
        .item(&item(app, "edit:find", "Find…", Some("CmdOrCtrl+F"))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item(app, "view:palette", "Command Palette…", Some("CmdOrCtrl+K"))?)
        .item(&item(app, "view:nav", "Headings", None)?)
        .item(&sep()?)
        .item(&item(app, "view:zoom_in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item(app, "view:zoom_out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item(app, "view:zoom_reset", "Actual Size", Some("CmdOrCtrl+0"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;
    let insert = SubmenuBuilder::new(app, "Insert")
        .item(&item(app, "insert:page_break", "Page Break", Some("CmdOrCtrl+Enter"))?)
        .item(&item(app, "insert:table", "Table", None)?)
        .item(&item(app, "insert:picture", "Picture…", None)?)
        .item(&sep()?)
        .item(&item(app, "insert:link", "Link…", Some("CmdOrCtrl+Alt+K"))?)
        .item(&item(app, "insert:footnote", "Footnote", None)?)
        .item(&item(app, "insert:toc", "Table of Contents", None)?)
        .build()?;
    let format = SubmenuBuilder::new(app, "Format")
        .item(&item(app, "format:bold", "Bold", Some("CmdOrCtrl+B"))?)
        .item(&item(app, "format:italic", "Italic", Some("CmdOrCtrl+I"))?)
        .item(&item(app, "format:underline", "Underline", Some("CmdOrCtrl+U"))?)
        .item(&sep()?)
        .item(&item(app, "format:bullets", "Bulleted List", None)?)
        .item(&item(app, "format:numbering", "Numbered List", None)?)
        .item(&sep()?)
        .item(&item(app, "format:left", "Align Left", Some("CmdOrCtrl+L"))?)
        .item(&item(app, "format:center", "Center", Some("CmdOrCtrl+E"))?)
        .item(&item(app, "format:right", "Align Right", Some("CmdOrCtrl+R"))?)
        .item(&item(app, "format:justify", "Justify", Some("CmdOrCtrl+J"))?)
        .build()?;
    let review = SubmenuBuilder::new(app, "Review")
        .item(&item(app, "review:comment", "New Comment", Some("CmdOrCtrl+Alt+M"))?)
        .item(&item(app, "review:panel", "Review Panel", None)?)
        .item(&sep()?)
        .item(&item(app, "review:track", "Track Changes", Some("CmdOrCtrl+Shift+E"))?)
        .item(&item(app, "review:accept_all", "Accept All Changes", None)?)
        .item(&item(app, "review:reject_all", "Reject All Changes", None)?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, None)?)
        .build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &insert, &format, &review, &window])
}

/// Files macOS asked us to open (Finder's Open With, double-click, dropping on the Dock
/// icon). They arrive as an event, not as arguments, possibly before the UI has loaded,
/// so they wait here until the UI takes them.
#[derive(Default)]
struct OpenedFiles(Mutex<Vec<String>>);

#[tauri::command]
fn take_opened(opened: State<'_, OpenedFiles>) -> Vec<String> {
    std::mem::take(&mut *opened.0.lock().unwrap())
}

fn main() {
    tauri::Builder::default()
        .manage(OpenedFiles::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let mut session = Session::new();
            // `quire path/to/file.docx` opens that file
            if let Some(path) = std::env::args().skip(1).find(|a| a.to_lowercase().ends_with(".docx") || a.to_lowercase().ends_with(".docm")) {
                if let Err(e) = session.open(std::path::Path::new(&path)) {
                    eprintln!("could not open {path}: {e}");
                }
            }
            app.manage(Mutex::new(session));
            std::thread::spawn(docs_core::spell::warm);
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            let _ = app.emit("menu", event.id().0.clone());
        })
        .invoke_handler(tauri::generate_handler![api, print_window, take_opened])
        .build(tauri::generate_context!())
        .expect("error while building Quire")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths: Vec<String> = urls.iter().filter_map(|u| u.to_file_path().ok()).map(|p| p.to_string_lossy().into_owned()).collect();
                if !paths.is_empty() {
                    _app.state::<OpenedFiles>().0.lock().unwrap().extend(paths);
                    let _ = _app.emit("opened", ());
                    if let Some(w) = _app.get_webview_window("main") {
                        let _ = w.unminimize();
                        let _ = w.set_focus();
                    }
                }
            }
        });
}
