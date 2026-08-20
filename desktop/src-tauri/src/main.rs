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
mod poll;

use std::sync::Mutex;

use myco_core::AppRuntime;

/// The one runtime behind every command and the poll thread.
pub struct Core(pub Mutex<AppRuntime>);

fn main() {
    let choice = backend::detect();
    eprintln!("myco-desktop: mesh backend: {choice}");
    let runtime = AppRuntime::with_config(choice.runtime_config());

    tauri::Builder::default()
        .manage(Core(Mutex::new(runtime)))
        .invoke_handler(tauri::generate_handler![
            commands::dispatch,
            commands::get_state
        ])
        .setup(|app| {
            poll::spawn(tauri::AppHandle::clone(app.handle()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri: event loop failed to start");
}
