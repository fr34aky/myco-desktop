//! The shell's half of paired file transfer (Android `MainActivity` parity).
//!
//! The core does the whole protocol — offer, consent, encrypt, fetch, decrypt —
//! and leaves two things to the shell: picking the files to send, and moving a
//! finished receive out of its private `received/` staging dir into somewhere
//! the user can see. On the phone that is MediaStore; here it is
//! `~/Downloads/Myco`, the same place the LAN share drops uploads.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::lanshare::free_path;
use crate::Core;

/// Receives already moved to Downloads, so a snapshot that still carries the
/// row (the forget is synchronous, but a poll can race a command) is not
/// published twice.
#[derive(Default)]
pub struct Published(Mutex<HashSet<String>>);

/// Move every decrypted receive the core has finished into `~/Downloads/Myco`,
/// then forget the row — which also deletes the plaintext staging copy. The
/// window hears `file-received` with the final path.
pub fn publish_received(app: &AppHandle, snapshot: &str) {
    let Ok(state) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return;
    };
    let Some(transfers) = state["fileTransfers"].as_array() else {
        return;
    };
    for t in transfers {
        let completed = t["direction"] == "incoming"
            && t["status"] == "completed"
            && t["publishPending"].as_bool().unwrap_or(false);
        let staged = t["receivedPath"].as_str().unwrap_or("");
        if !completed || staged.is_empty() {
            continue;
        }
        let id = t["id"].as_str().unwrap_or("").to_string();
        let published = app.state::<Published>();
        if !published.0.lock().unwrap().insert(id.clone()) {
            continue;
        }
        let name = t["name"].as_str().unwrap_or("received.bin");
        let destination = free_path(name);
        if let Some(dir) = destination.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = move_file(Path::new(staged), &destination) {
            tracing::warn!(%error, path = staged, "file share: could not publish received file");
            // Leave the row: the plaintext is still in staging and the user
            // can find it via the error below rather than lose the file.
            let _ = app.emit(
                "file-received",
                serde_json::json!({ "name": name, "path": staged, "error": error.to_string() }),
            );
            continue;
        }
        let core = app.state::<Core>();
        let fresh = core.0.lock().unwrap().dispatch_json(
            &serde_json::json!({ "type": "forget_file_transfer", "transferId": id }).to_string(),
        );
        let _ = app.emit("state", &fresh);
        let _ = app.emit(
            "file-received",
            serde_json::json!({
                "name": name,
                "path": destination.to_string_lossy(),
                "from": t["peerName"].as_str().unwrap_or(""),
            }),
        );
    }
}

/// Rename where possible; `~/Downloads` is often another filesystem from the
/// data dir, so fall back to copy + delete.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Native file picker, then one `share_file` per pick. The core reads the
/// path into its own encrypted outbox and never deletes a file outside its
/// Android staging dir, so the originals are passed as-is. Returns the state
/// after the last dispatch, or `None` if the picker was cancelled.
pub fn pick_and_share(app: &AppHandle, peer_npub: &str) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_files()?;
    let core = app.state::<Core>();
    let mut latest = None;
    for file in picked {
        let Ok(path) = file.into_path() else { continue };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "shared-file".to_string());
        let action = serde_json::json!({
            "type": "share_file",
            "path": path.to_string_lossy(),
            "name": name,
            "mime": mime_for(&name),
            "peerNpub": peer_npub,
        });
        latest = Some(core.0.lock().unwrap().dispatch_json(&action.to_string()));
    }
    latest
}

/// Enough of a MIME guess for the offer card on the other side; the receiver
/// derives its own from the extension anyway, and an empty string would read
/// as octet-stream.
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}
