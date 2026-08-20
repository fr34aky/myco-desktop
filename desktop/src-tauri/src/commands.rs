//! The shell UI's two doors into the reducer — the desktop `dispatchJson`.

use tauri::State;

use crate::Core;

/// Reduce one action (the same snake_case JSON the Android client builds)
/// and return the fresh state snapshot.
#[tauri::command]
pub fn dispatch(core: State<'_, Core>, action: String) -> String {
    core.0.lock().unwrap().dispatch_json(&action)
}

/// The current state snapshot without reducing anything.
#[tauri::command]
pub fn get_state(core: State<'_, Core>) -> String {
    core.0.lock().unwrap().state_json()
}
