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
        if app.emit("state", &snapshot).is_err() {
            // The event loop is gone; so is any reason to keep polling.
            return;
        }
    });
}
