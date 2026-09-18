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
mod filetransfer;
mod gateway_http;
mod lanshare;
mod napplets;
mod nsite_windows;
mod pairing;
mod poll;

use std::sync::Mutex;

use myco_core::AppRuntime;
use tauri::Manager;

/// The one runtime behind every command and the poll thread.
pub struct Core(pub Mutex<AppRuntime>);

const DMABUF_VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// What to set [`DMABUF_VAR`] to, if anything: `1` where NVIDIA's own driver
/// is loaded, unless the environment already says something.
///
/// WebKitGTK's DMABUF renderer needs GBM, and with that driver it does not
/// get it. Three faces of one fault, all seen on one hybrid Intel+NVIDIA box
/// — so the driver being loaded is the test, not which GPU drives the screen:
/// the AppImage's bundled WebKit logs "Could not create GBM EGL display:
/// EGL_SUCCESS. Aborting..." and aborts before the first window (its hook
/// forces `GDK_BACKEND=x11`, tauri#8541); a newer host WebKit under X11 stays
/// up over a window it never paints; under native Wayland it dies on "Error
/// 71 (Protocol error)". The narrower `WEBKIT_DMABUF_RENDERER_DISABLE_GBM`
/// cures only the first, which is why the whole renderer goes.
///
/// A value already in the environment wins, whatever it is: WebKit reads `0`
/// as "leave the renderer on", the way back for a driver that has since
/// learned to do this. Intel and AMD machines are never touched.
fn dmabuf_override(
    existing: Option<&std::ffi::OsStr>,
    nvidia_driver: bool,
) -> Option<&'static str> {
    (existing.is_none() && nvidia_driver).then_some("1")
}

/// Apply [`dmabuf_override`]. Call it first thing in `main`: `set_var` is only
/// sound while the process has one thread — GTK, GLib and the tokio runtime
/// all `getenv` from theirs — and that is a stricter bound than "before the
/// first webview", which is all WebKit itself asks.
fn disable_dmabuf_renderer_on_nvidia() {
    // Only NVIDIA's driver creates this, never nouveau. A /proc we cannot
    // read (a sandbox masking it) is not "no NVIDIA"; say so, since the
    // abort that follows names nothing.
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version")
        .try_exists()
        .unwrap_or_else(|e| {
            eprintln!("myco-desktop: cannot tell whether the NVIDIA driver is loaded ({e}); if the window stays empty or the app aborts, set {DMABUF_VAR}=1");
            false
        });
    if let Some(value) = dmabuf_override(std::env::var_os(DMABUF_VAR).as_deref(), nvidia) {
        std::env::set_var(DMABUF_VAR, value);
        eprintln!("myco-desktop: NVIDIA driver loaded, WebKit's DMABUF renderer is off ({DMABUF_VAR}={value})");
    }
}

fn main() {
    disable_dmabuf_renderer_on_nvidia();

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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::dispatch,
            commands::get_state,
            commands::open_nsite_window,
            commands::open_napplet_window,
            commands::pair_payload,
            commands::handle_link,
            commands::invite_peer,
            commands::device_name,
            commands::rename_device,
            commands::lanshare_start,
            commands::lanshare_stop,
            commands::lanshare_status,
            commands::lanshare_send_files,
            commands::lanshare_decide,
            commands::ui_log,
            commands::backend_info,
            commands::share_payload,
            commands::add_launcher_shortcut,
            commands::share_file_with_peer
        ])
        .setup(|app| {
            let choice = match backend::detect() {
                Ok(choice) => choice,
                // A refusal (forced backend that cannot work here) is worded
                // for humans; show it and stop — there is nothing to run.
                Err(message) => {
                    use tauri_plugin_dialog::DialogExt;
                    app.dialog()
                        .message(&message)
                        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                        .title("Myco cannot start")
                        .blocking_show();
                    eprintln!("myco-desktop: {message}");
                    std::process::exit(1);
                }
            };
            eprintln!("myco-desktop: mesh backend: {choice}");
            let config = choice.runtime_config();
            let pairing = pairing::Pairing::new(&config.data_dir);
            let mut runtime = AppRuntime::with_config(config);

            // The nsite gateway and the LAN share ride the core's own tokio
            // runtime; without a content layer (startup error) neither runs.
            let share = std::sync::Arc::new(lanshare::LanShare::default());
            if let Some((content, handle)) = runtime.gateway_context() {
                gateway_http::spawn(
                    tauri::AppHandle::clone(app.handle()),
                    content,
                    handle.clone(),
                );
                share.attach(tauri::AppHandle::clone(app.handle()), handle);
            }
            // The napplet host rides the same content layer: the gateway
            // serves its shell pages and carries its capability channel.
            if let Some((host, _)) = runtime.napplet_context() {
                app.manage(std::sync::Arc::new(napplets::Napplets::new(host)));
            }
            app.manage(std::sync::Arc::clone(&share));

            // Stamp our memorable name onto outgoing pair events from the
            // start — Android does the same at launch. No-op in degraded
            // daemon mode.
            let identity = serde_json::from_str::<serde_json::Value>(&runtime.state_json())
                .map(|s| s["identity"].clone())
                .unwrap_or_default();
            let own_npub = identity["ownNpub"].as_str().unwrap_or_default().to_string();
            if !own_npub.is_empty() {
                let name = pairing.device_name(&own_npub);
                runtime.dispatch_json(
                    &serde_json::json!({"type": "set_device_name", "name": name}).to_string(),
                );
            }
            // Mesh-mode file sharing binds the mesh ULA and advertises the
            // .fips name; without an identity (degraded daemon mode) the
            // share falls back to erroring out with a clear message.
            let (ipv6, fips_addr) = (
                identity["fipsIpv6"].as_str().unwrap_or_default(),
                identity["fipsAddr"].as_str().unwrap_or_default(),
            );
            if !ipv6.is_empty() && !fips_addr.is_empty() {
                share.set_mesh(ipv6.to_string(), fips_addr.to_string());
            }

            // An embedded node is the mesh — bring it up with the app, the
            // way Android does. (Daemon mode embeds no node; StartNode there
            // is only the systemd hint.)
            if matches!(choice, backend::Choice::Embedded { .. }) {
                runtime.dispatch_json(&serde_json::json!({"type": "start_node"}).to_string());
            }

            app.manage(Core(Mutex::new(runtime)));
            app.manage(pairing);
            app.manage(choice);
            app.manage(filetransfer::Published::default());
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

#[cfg(test)]
mod tests {
    use super::dmabuf_override;
    use std::ffi::OsStr;

    #[test]
    fn the_nvidia_driver_turns_the_dmabuf_renderer_off() {
        assert_eq!(dmabuf_override(None, true), Some("1"));
    }

    #[test]
    fn other_gpus_keep_the_dmabuf_renderer() {
        assert_eq!(dmabuf_override(None, false), None);
    }

    #[test]
    fn a_value_already_in_the_environment_wins() {
        // `0` is WebKit's "leave it on" — the opt-out — and an empty value
        // is still the user having said something.
        for existing in ["0", "1", ""] {
            assert_eq!(dmabuf_override(Some(OsStr::new(existing)), true), None);
        }
    }
}
