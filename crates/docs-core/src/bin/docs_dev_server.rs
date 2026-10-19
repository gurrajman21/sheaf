//! Development bridge: serves `Session::dispatch` over HTTP so the UI can run in
//! a normal browser (Vite proxies `/api` here) against the real Rust backend.
//! The desktop app doesn't use this; it calls `dispatch` through Tauri.
//!
//! Usage: docs_dev_server [port] [file.docx]

docs_core::fast_oxi_env!();

use docs_core::Session;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(7879u16);
    let mut session = Session::new();
    if let Some(file) = args.get(2) {
        if let Err(e) = session.open(std::path::Path::new(file)) {
            eprintln!("could not open {file}: {e}");
        }
    }
    std::thread::spawn(docs_core::spell::warm);
    let server = Server::http(("127.0.0.1", port)).expect("bind");
    eprintln!("docs dev server on http://127.0.0.1:{port}");
    let json_header = Header::from_bytes("Content-Type", "application/json").unwrap();
    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        let reply = match (req.method(), url.strip_prefix("/api/")) {
            (Method::Post, Some(cmd)) => {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let args: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let t = std::time::Instant::now();
                let r = session.dispatch(cmd, &args);
                eprintln!("[api] {cmd} {:.0}ms{}", t.elapsed().as_secs_f64() * 1000.0, r.as_ref().err().map(|e| format!(" ERR {e}")).unwrap_or_default());
                match r {
                    Ok(data) => json!({"ok": true, "data": data}),
                    Err(error) => json!({"ok": false, "error": error}),
                }
            }
            _ => json!({"ok": false, "error": "POST /api/<command>"}),
        };
        let _ = req.respond(Response::from_string(reply.to_string()).with_header(json_header.clone()));
    }
}
