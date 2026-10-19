//! Autosave, crash recovery and version history, stored in the app's data folder
//! (`~/Library/Application Support/Quire` on macOS, `$XDG_DATA_HOME/quire` or
//! `~/.local/share/quire` on Linux, `%APPDATA%\Quire` on Windows).
//!
//! - `recovery/<key>.docx` + `.json`: the latest autosave of a document with unsaved
//!   changes; removed when it's saved or closed cleanly.
//! - `versions/<key>/<unix-millis>.docx`: snapshots taken on save and every few minutes
//!   while editing; the newest 40 per document are kept.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const KEEP_VERSIONS: usize = 40;

pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("QUIRE_DATA_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/Quire")
    } else if cfg!(windows) {
        std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(home)).join("Quire")
    } else {
        std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(home).join(".local/share")).join("quire")
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Stable key for a document: its path, or a per-session id for unsaved documents.
pub fn key_for(path: Option<&Path>, session_id: &str) -> String {
    match path {
        Some(p) => {
            let s = p.to_string_lossy();
            let mut h: u64 = 0xcbf29ce484222325;
            for b in s.bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
            format!("{h:016x}")
        }
        None => format!("untitled-{session_id}"),
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

pub fn save_recovery(key: &str, bytes: &[u8], name: &str, path: Option<&Path>) -> Result<(), String> {
    let dir = data_dir().join("recovery");
    write_atomic(&dir.join(format!("{key}.docx")), bytes).map_err(|e| e.to_string())?;
    let meta = json!({"key": key, "name": name, "path": path.map(|p| p.display().to_string()), "at": now_ms() as u64});
    write_atomic(&dir.join(format!("{key}.json")), meta.to_string().as_bytes()).map_err(|e| e.to_string())
}

pub fn clear_recovery(key: &str) {
    let dir = data_dir().join("recovery");
    let _ = std::fs::remove_file(dir.join(format!("{key}.docx")));
    let _ = std::fs::remove_file(dir.join(format!("{key}.json")));
}

pub fn list_recovery() -> Vec<Value> {
    let dir = data_dir().join("recovery");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<Value> = entries.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()))
        .filter(|v| dir.join(format!("{}.docx", v["key"].as_str().unwrap_or(""))).exists())
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v["at"].as_u64().unwrap_or(0)));
    out
}

pub fn recovery_bytes(key: &str) -> Result<Vec<u8>, String> {
    std::fs::read(data_dir().join("recovery").join(format!("{key}.docx"))).map_err(|e| e.to_string())
}

/// Snapshot `bytes` as a version of `key`, unless the newest version is identical.
pub fn save_version(key: &str, bytes: &[u8], reason: &str) -> Result<(), String> {
    let dir = data_dir().join("versions").join(key);
    let existing = list_versions(key);
    if let Some(last) = existing.first() {
        if std::fs::read(dir.join(format!("{}.docx", last["id"].as_str().unwrap_or("")))).is_ok_and(|b| b == bytes) {
            return Ok(());
        }
    }
    let id = now_ms().to_string();
    write_atomic(&dir.join(format!("{id}.docx")), bytes).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(format!("{id}.json")), json!({"reason": reason}).to_string().as_bytes()).map_err(|e| e.to_string())?;
    for old in list_versions(key).iter().skip(KEEP_VERSIONS) {
        let id = old["id"].as_str().unwrap_or("");
        let _ = std::fs::remove_file(dir.join(format!("{id}.docx")));
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    }
    Ok(())
}

/// Versions of `key`, newest first: {id, at, reason, size}.
pub fn list_versions(key: &str) -> Vec<Value> {
    let dir = data_dir().join("versions").join(key);
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<Value> = entries.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "docx"))
        .filter_map(|p| {
            let id = p.file_stem()?.to_string_lossy().to_string();
            let at: u64 = id.parse().ok()?;
            let reason = std::fs::read_to_string(p.with_extension("json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v["reason"].as_str().map(str::to_string)).unwrap_or_default();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some(json!({"id": id, "at": at, "reason": reason, "size": size}))
        })
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v["at"].as_u64().unwrap_or(0)));
    out
}

pub fn version_bytes(key: &str, id: &str) -> Result<Vec<u8>, String> {
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("bad version id".into());
    }
    std::fs::read(data_dir().join("versions").join(key).join(format!("{id}.docx"))).map_err(|e| e.to_string())
}
