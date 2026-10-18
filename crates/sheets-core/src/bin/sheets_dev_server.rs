//! Development bridge: serves `Session::dispatch` over HTTP so the UI can run in
//! a normal browser (Vite proxies `/api` here) against the real Rust backend.
//! The desktop app doesn't use this; it calls `dispatch` through Tauri.
//!
//! Usage: sheets_dev_server [port] [file.xlsx]
//! POST /api/<command> with a JSON body of arguments ->
//!   {"ok": true, "data": ...} or {"ok": false, "error": "..."}

use serde_json::{json, Value};
use sheets_core::Session;
use tiny_http::{Header, Method, Response, Server};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(7878u16);
    let mut session = Session::new();
    if let Some(file) = args.get(2) {
        if let Err(e) = session.open(std::path::Path::new(file)) {
            eprintln!("could not open {file}: {e}");
        }
    }
    let server = Server::http(("127.0.0.1", port)).expect("bind");
    eprintln!("sheets dev server on http://127.0.0.1:{port}");
    let json_header = Header::from_bytes("Content-Type", "application/json").unwrap();
    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        let reply = match (req.method(), url.strip_prefix("/api/")) {
            (Method::Post, Some(cmd)) => {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let args: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                match session.dispatch(cmd, &args) {
                    Ok(data) => json!({"ok": true, "data": data}),
                    Err(error) => json!({"ok": false, "error": error}),
                }
            }
            _ => json!({"ok": false, "error": "POST /api/<command>"}),
        };
        let _ = req.respond(Response::from_string(reply.to_string()).with_header(json_header.clone()));
    }
}
