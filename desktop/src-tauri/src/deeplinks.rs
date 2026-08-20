//! `myco://` links — deep-linked, pasted, or forwarded from a second launch.
//!
//! The desktop analogue of Android's `handleScannedText`: pair links send a
//! pair request, share links additionally open the shared nsite from the
//! sharer, app links open the named nsite, and anything else is treated as a
//! pasted nsite link. Unlike Android there is no pending-deep-link store: a
//! deep link always arrives in a *running* instance (single-instance forwards
//! the argv), so it is acted on immediately and the 503 loading page covers
//! the sync window.

use tauri::{AppHandle, Emitter, Manager};

use crate::pairing::{Link, PairInfo};
use crate::Core;

pub fn handle(app: &AppHandle, text: &str) {
    match crate::pairing::parse_link(text) {
        Link::Pair(info) => {
            send_pair_request(app, &info);
            goto(app, "circle");
        }
        Link::Share { nsite, pair } => {
            send_pair_request(app, &pair);
            dispatch(
                app,
                serde_json::json!({"type": "open_nsite", "link": nsite, "holder": pair.npub}),
            );
            let _ = crate::nsite_windows::open(app, &nsite, &nsite);
        }
        Link::App { host } => {
            dispatch(app, serde_json::json!({"type": "open_nsite", "link": host}));
            let _ = crate::nsite_windows::open(app, &host, &host);
        }
        Link::Raw(link) => {
            if link.is_empty() {
                return;
            }
            dispatch(app, serde_json::json!({"type": "open_nsite", "link": link}));
            goto(app, "apps");
        }
    }
}

fn send_pair_request(app: &AppHandle, info: &PairInfo) {
    dispatch(
        app,
        serde_json::json!({
            "type": "send_pair_request",
            "npub": info.npub,
            "name": info.name,
            "secret": info.secret,
        }),
    );
}

fn dispatch(app: &AppHandle, action: serde_json::Value) {
    // try_state: a forwarded link can theoretically arrive in the gap before
    // setup has managed the runtime — dropping it beats panicking the IPC
    // callback.
    let Some(core) = app.try_state::<Core>() else {
        tracing::warn!(%action, "deep link arrived before the runtime was up; dropped");
        return;
    };
    let fresh = core.0.lock().unwrap().dispatch_json(&action.to_string());
    let _ = app.emit("state", &fresh);
}

/// Surface the main window on the named tab — a deep link should land the
/// user where its effect is visible.
fn goto(app: &AppHandle, tab: &str) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_focus();
    }
    let _ = app.emit("goto", tab);
}
