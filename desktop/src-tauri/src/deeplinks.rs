//! `myco://` links — deep-linked, pasted, or forwarded from a second launch.
//!
//! The desktop analogue of Android's `handleScannedText`: pair links send a
//! pair request, share links additionally open the shared nsite from the
//! sharer — or, for a napplet, fetch it from the sharer's device and put it
//! up for install review — app links open the named nsite, napplet links
//! open an installed napplet, an `naddr` is fetched for review, and anything
//! else is treated as a pasted nsite link. An `naddr` never installs on its
//! own: Rust reports what it asks for and the review sheet is the only place
//! a grant is written. Unlike Android there is no pending-deep-link store: a
//! deep link always arrives in a *running* instance (single-instance forwards
//! the argv), so it is acted on immediately and the 503 loading page covers
//! the sync window.

use tauri::{AppHandle, Emitter, Manager};

use crate::pairing::{Link, PairInfo, SharedApp};
use crate::Core;

pub fn handle(app: &AppHandle, text: &str) {
    match crate::pairing::parse_link(text) {
        Link::Pair(info) => {
            send_pair_request(app, &info);
            goto(app, "circle");
        }
        Link::Share {
            app: SharedApp::Nsite(nsite),
            pair,
        } => {
            send_pair_request(app, &pair);
            dispatch(
                app,
                serde_json::json!({"type": "open_nsite", "link": nsite, "holder": pair.npub}),
            );
            let _ = crate::nsite_windows::open(app, &nsite, &nsite);
        }
        Link::Share {
            app: SharedApp::Napplet(pointer),
            pair,
        } => {
            // The sharer's device is tried before the internet, so a napplet
            // handed over in a room with no internet still arrives.
            send_pair_request(app, &pair);
            dispatch(
                app,
                serde_json::json!({"type": "fetch_napplet", "pointer": pointer, "holder": pair.npub}),
            );
            goto(app, "apps");
        }
        Link::App { host } => {
            dispatch(app, serde_json::json!({"type": "open_nsite", "link": host}));
            let _ = crate::nsite_windows::open(app, &host, &host);
        }
        Link::Napplet { pointer } => open_napplet(app, &pointer),
        Link::Raw(link) => {
            if link.is_empty() {
                return;
            }
            if is_naddr(&link) {
                dispatch(
                    app,
                    serde_json::json!({"type": "fetch_napplet", "pointer": link}),
                );
            } else {
                dispatch(app, serde_json::json!({"type": "open_nsite", "link": link}));
            }
            goto(app, "apps");
        }
    }
}

/// `naddr1…` is the honest signal for a napplet: it names a kind, and Rust
/// refuses one that is not a napplet kind, so a mis-tagged `naddr` fails
/// there with a clear reason rather than being tried as an nsite host.
fn is_naddr(text: &str) -> bool {
    text.len() > 6 && text[..6].eq_ignore_ascii_case("naddr1")
}

/// A launcher shortcut to a napplet: open it if it is installed, else fetch
/// it for review — the shortcut may outlive the install.
fn open_napplet(app: &AppHandle, pointer: &str) {
    let Some(core) = app.try_state::<Core>() else {
        tracing::warn!(
            pointer,
            "napplet link arrived before the runtime was up; dropped"
        );
        return;
    };
    let installed = {
        let state: serde_json::Value =
            serde_json::from_str(&core.0.lock().unwrap().state_json()).unwrap_or_default();
        state["library"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| {
                item["kind"] == "napplet" && item["pinned"] == true && item["pointer"] == pointer
            })
            .map(|item| {
                (
                    item["urlHost"].as_str().unwrap_or_default().to_string(),
                    item["title"].as_str().unwrap_or_default().to_string(),
                )
            })
    };
    match installed {
        Some((host, title)) => {
            let app = app.clone();
            let pointer = pointer.to_string();
            // The resolve blocks; the link handler runs on the event loop.
            std::thread::spawn(move || {
                if let Err(e) =
                    crate::napplets::Napplets::open_window(&app, &host, &pointer, &title)
                {
                    tracing::warn!(pointer, error = %e, "napplet link did not open");
                }
            });
        }
        None => {
            dispatch(
                app,
                serde_json::json!({"type": "fetch_napplet", "pointer": pointer}),
            );
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
