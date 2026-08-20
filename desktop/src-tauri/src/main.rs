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
mod gateway_http;
mod nsite_windows;
mod poll;

use std::sync::Mutex;

use myco_core::AppRuntime;

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

    let choice = backend::detect();
    eprintln!("myco-desktop: mesh backend: {choice}");
    let runtime = AppRuntime::with_config(choice.runtime_config());

    // The nsite gateway rides the core's own tokio runtime; without a content
    // layer (startup error) there is nothing to serve.
    if let Some((content, handle)) = runtime.gateway_context() {
        gateway_http::spawn(content, handle);
    }

    tauri::Builder::default()
        .manage(Core(Mutex::new(runtime)))
        .invoke_handler(tauri::generate_handler![
            commands::dispatch,
            commands::get_state,
            commands::open_nsite_window
        ])
        .setup(|app| {
            poll::spawn(tauri::AppHandle::clone(app.handle()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri: event loop failed to start");
}
