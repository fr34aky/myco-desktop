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

/// Open (or re-focus) the window for one nsite.
#[tauri::command]
pub fn open_nsite_window(app: tauri::AppHandle, host: String, title: String) -> Result<(), String> {
    crate::nsite_windows::open(&app, &host, &title)
}

/// The "My code" payload: the current `myco://pair` URI and its QR as SVG.
#[tauri::command]
pub fn pair_payload(
    core: State<'_, Core>,
    pairing: State<'_, crate::pairing::Pairing>,
) -> Result<serde_json::Value, String> {
    let npub = {
        let mut runtime = core.0.lock().unwrap();
        let state: serde_json::Value =
            serde_json::from_str(&runtime.state_json()).map_err(|e| e.to_string())?;
        state["identity"]["ownNpub"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    if npub.is_empty() {
        return Err("no identity yet (degraded mode?)".to_string());
    }
    let name = pairing.device_name(&npub);
    let uri = pairing.pair_uri(&npub, &name);
    let svg = qrcode::QrCode::new(uri.as_bytes())
        .map_err(|e| e.to_string())?
        .render()
        .min_dimensions(260, 260)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Ok(serde_json::json!({ "uri": uri, "svg": svg, "name": name }))
}

/// Handle a pasted code or link — same routing as a deep link.
#[tauri::command]
pub fn handle_link(app: tauri::AppHandle, text: String) {
    crate::deeplinks::handle(&app, &text);
}

/// Invite a connected peer: mint a fresh secret (Android parity — invite
/// secrets are one-shot and unledgered) and send the request.
#[tauri::command]
pub fn invite_peer(core: State<'_, Core>, npub: String, name: String) -> String {
    let action = serde_json::json!({
        "type": "send_pair_request",
        "npub": npub,
        "name": name,
        "secret": crate::pairing::new_secret(),
    });
    core.0.lock().unwrap().dispatch_json(&action.to_string())
}

/// This device's memorable name (for the Circle identity chip).
#[tauri::command]
pub fn device_name(core: State<'_, Core>, pairing: State<'_, crate::pairing::Pairing>) -> String {
    let mut runtime = core.0.lock().unwrap();
    let npub = serde_json::from_str::<serde_json::Value>(&runtime.state_json())
        .ok()
        .and_then(|s| s["identity"]["ownNpub"].as_str().map(str::to_string))
        .unwrap_or_default();
    pairing.device_name(&npub)
}

/// Rename this device: persist and stamp future outgoing pair events.
#[tauri::command]
pub fn rename_device(
    core: State<'_, Core>,
    pairing: State<'_, crate::pairing::Pairing>,
    name: String,
) -> String {
    pairing.set_device_name(&name);
    let action = serde_json::json!({"type": "set_device_name", "name": name.trim()});
    core.0.lock().unwrap().dispatch_json(&action.to_string())
}
