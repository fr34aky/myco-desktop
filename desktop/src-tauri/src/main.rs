//! Myco desktop shell.
//!
//! One Tauri process holding a `Mutex<AppRuntime>` — the same shape the JNI
//! handle wraps on Android, minus the FFI: the shell UI drives the reducer
//! through the `dispatch`/`get_state` commands, and a 1 Hz poll thread pushes
//! every state snapshot to the window as a `state` event (the reducer's
//! `poll_pending_start` rides that same cadence). docs/design/desktop.md.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod commands;
mod deeplinks;
mod gateway_http;
mod nsite_windows;
mod pairing;
mod poll;

use std::sync::Mutex;

use myco_core::AppRuntime;
use tauri::Manager;

/// The one runtime behind every command and the poll thread.
pub struct Core(pub Mutex<AppRuntime>);

fn main() {
    // The core speaks tracing; without a subscriber every sync failure is
    // silent. RUST_LOG overrides; info is the useful default.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        // A second launch — most importantly `xdg-open myco://…` — forwards
        // its argv here and exits. Registered first and everything heavy done
        // in setup, so a second instance never constructs a runtime or opens
        // the stores the first instance owns.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            for arg in args.iter().skip(1) {
                if arg.starts_with("myco://") {
                    deeplinks::handle(app, arg);
                }
            }
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_focus();
            }
        }))
        .invoke_handler(tauri::generate_handler![
            commands::dispatch,
            commands::get_state,
            commands::open_nsite_window,
            commands::pair_payload,
            commands::handle_link,
            commands::invite_peer,
            commands::device_name,
            commands::rename_device
        ])
        .setup(|app| {
            let choice = backend::detect();
            eprintln!("myco-desktop: mesh backend: {choice}");
            let config = choice.runtime_config();
            let pairing = pairing::Pairing::new(&config.data_dir);
            let mut runtime = AppRuntime::with_config(config);

            // The nsite gateway rides the core's own tokio runtime; without a
            // content layer (startup error) there is nothing to serve.
            if let Some((content, handle)) = runtime.gateway_context() {
                gateway_http::spawn(content, handle);
            }

            // Stamp our memorable name onto outgoing pair events from the
            // start — Android does the same at launch. No-op in degraded
            // daemon mode.
            let own_npub = serde_json::from_str::<serde_json::Value>(&runtime.state_json())
                .ok()
                .and_then(|s| s["identity"]["ownNpub"].as_str().map(str::to_string))
                .unwrap_or_default();
            if !own_npub.is_empty() {
                let name = pairing.device_name(&own_npub);
                runtime.dispatch_json(
                    &serde_json::json!({"type": "set_device_name", "name": name}).to_string(),
                );
            }

            app.manage(Core(Mutex::new(runtime)));
            app.manage(pairing);
            poll::spawn(tauri::AppHandle::clone(app.handle()));

            // A cold start via `xdg-open myco://…` carries the link in argv.
            let handle = tauri::AppHandle::clone(app.handle());
            for arg in std::env::args().skip(1) {
                if arg.starts_with("myco://") {
                    deeplinks::handle(&handle, &arg);
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri: event loop failed to start");
}
