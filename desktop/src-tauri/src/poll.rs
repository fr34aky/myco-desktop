//! The 1 Hz state poll.
//!
//! Required, not decorative: `AppRuntime::poll_pending_start` replays a
//! queued node start on this cadence (on Android the UI's 1 Hz poll is the
//! clock). Every snapshot is pushed to the window — peer rows and daemon
//! status change without a `rev` bump, so filtering on `rev` would freeze
//! exactly the data this screen exists to show; at 1 Hz the wasted renders
//! are noise.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::Core;

pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let snapshot = {
            let core = app.state::<Core>();
            let mut runtime = core.0.lock().unwrap();
            runtime.state_json()
        };
        auto_accept(&app, &snapshot);
        crate::filetransfer::publish_received(&app, &snapshot);
        if app.emit("state", &snapshot).is_err() {
            // The event loop is gone; so is any reason to keep polling.
            return;
        }
    });
}

/// The presenter-side half of QR pairing (Android `MycoApp` parity): an
/// incoming request whose secret matches one this device issued is accepted
/// silently — the peer proved they scanned *our* code — and the code rotates
/// (consume clears it; the UI re-fetches on the `pair-rotated` event). A
/// request with no matching secret stays in "waiting to join" for a human.
fn auto_accept(app: &AppHandle, snapshot: &str) {
    let Ok(state) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return;
    };
    let Some(pending) = state["pendingPairRequests"].as_array() else {
        return;
    };
    let pairing = app.state::<crate::pairing::Pairing>();
    for request in pending {
        let secret = request["secret"].as_str().unwrap_or("");
        if !pairing.consume(secret) {
            continue;
        }
        let action = serde_json::json!({
            "type": "accept_pair_request",
            "npub": request["npub"].as_str().unwrap_or(""),
            "name": request["name"].as_str().unwrap_or(""),
        });
        let core = app.state::<Core>();
        let fresh = core.0.lock().unwrap().dispatch_json(&action.to_string());
        let _ = app.emit("state", &fresh);
        let _ = app.emit("pair-rotated", ());
    }
}
