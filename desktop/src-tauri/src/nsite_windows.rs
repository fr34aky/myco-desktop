//! One chrome-less window per open nsite — the desktop's NsiteActivity.
//!
//! The window is keyed by host, so re-opening a site surfaces its existing
//! window instead of stacking a second one (Android's `documentLaunchMode=
//! "intoExisting"` behavior). The page loads straight off the loopback
//! gateway; the OS title bar carries the site name and is all the chrome
//! there is.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::gateway_http::GATEWAY_PORT;

pub fn open(app: &AppHandle, host: &str, title: &str) -> Result<(), String> {
    let label = window_label(host);
    if let Some(existing) = app.get_webview_window(&label) {
        return existing.set_focus().map_err(|e| e.to_string());
    }

    let url: tauri::Url = format!("http://{host}.localhost:{GATEWAY_PORT}/")
        .parse()
        .map_err(|e| format!("bad nsite URL for {host:?}: {e}"))?;
    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(if title.is_empty() { host } else { title })
        .inner_size(1000.0, 760.0)
        .min_inner_size(360.0, 360.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Tauri window labels allow `[a-zA-Z0-9-/:_]`; host labels are bech32/base36
/// plus whatever a d-tag carries, so anything else maps to `-`. Good enough to
/// key windows — hosts differing only in exotic characters are theoretical.
fn window_label(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("nsite-{safe}")
}
