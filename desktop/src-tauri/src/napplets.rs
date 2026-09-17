//! Napplet windows and their capability channel — the desktop `NappletActivity`.
//!
//! ## What runs where
//!
//! ```text
//!   window  ->  the shell page at http://<label>.napplet.localhost:4880/  (ours, trusted)
//!                 └─ iframe sandbox="allow-scripts", srcdoc                (the napplet, untrusted)
//! ```
//!
//! Exactly the phone's shape: the napplet is never the window's top-level
//! document, only the opaque origin its sandboxed iframe gives it. The shell
//! page is the runtime crate's, byte for byte; what differs is how the
//! capability channel reaches it.
//!
//! ## The capability channel
//!
//! Android injects the runtime object with `addWebMessageListener`, scoped to
//! the shell origin and to nothing else. wry has no per-origin injection, and
//! Tauri IPC is deliberately not granted to nsite or napplet windows — their
//! pages are plain web content, like every nsite window. So the channel is
//! **loopback HTTP on the shell's own origin**: the gateway serves the shell
//! page with a prelude that defines `window.mycoNappletRuntime` over
//! `fetch("/__myco/napplet/<session>/frame")` and a long poll on `/next` —
//! the same frame/next-frames/close contract the JNI exposes, spoken over the
//! loopback socket the page already lives on.
//!
//! The channel token is minted here — random, never the core's session id,
//! which is a counter the phone never lets out of the process — written into
//! that one top-level document, and *is* the capability: the napplet's iframe
//! has an opaque origin and a `connect-src 'none'` CSP, and never sees it. A
//! session is opened per page load — the first load adopts the one `open_window`
//! resolved before showing anything (the phone resolves under the splash),
//! a reload after a grant change (`relaunch`) opens its own — and the previous
//! session of the same window is closed either way, so one window is ever
//! one napplet with one session. Closing the window closes its session.
//!
//! Verification, policy and every capability live in Rust. This module is a
//! pipe.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use myco_core::NappletHost;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::gateway_http::GATEWAY_PORT;
use crate::Core;

/// How long one drain call waits before answering empty — long enough that
/// an idle napplet is not spinning on the loopback, short enough that a
/// closed window's last poll is not held for long (`NappletActivity`'s
/// `DRAIN_WAIT_MS`).
const DRAIN_WAIT: Duration = Duration::from_secs(20);

/// The channel's path prefix on the shell origin; the session id follows.
const CHANNEL_PREFIX: &str = "/__myco/napplet/";

/// The desktop's napplet host: which window shows which napplet, and which
/// session belongs to which window.
pub struct Napplets {
    host: Arc<NappletHost>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Shell host → the napplet a window at that origin shows.
    windows: HashMap<String, WindowEntry>,
    /// Channel token → the session it drives and the shell host it was
    /// minted for. What the channel endpoints resolve a request through, and
    /// what a closing window uses to find its sessions.
    sessions: HashMap<String, SessionRef>,
}

#[derive(Clone)]
struct SessionRef {
    /// The core's session id (`NappletHost::open_with`).
    id: String,
    host: String,
}

struct WindowEntry {
    pointer: String,
    /// The token of a session `open_window` resolved that the page's first
    /// load adopts.
    pending: Option<String>,
    /// The token of the session the page currently runs.
    live: Option<String>,
}

impl Napplets {
    pub fn new(host: Arc<NappletHost>) -> Self {
        Self {
            host,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Open (or re-focus) the window for one installed napplet.
    ///
    /// Resolves and verifies **before** the window exists, exactly as the
    /// phone does under its splash: a napplet that fails any check gets no
    /// session and no window, and the error goes back to the Apps tab in
    /// words. Blocks on the resolve; call it off the UI thread.
    pub fn open_window(
        app: &AppHandle,
        host: &str,
        pointer: &str,
        title: &str,
    ) -> Result<(), String> {
        let label = window_label(host);
        if let Some(existing) = app.get_webview_window(&label) {
            return existing.set_focus().map_err(|e| e.to_string());
        }
        let napplets = app
            .try_state::<Arc<Napplets>>()
            .ok_or("napplets are unavailable (the content layer did not start)")?;

        let opened = open_session(app, pointer)?;
        // The origin the shell is served at is the napplet's, not what the
        // caller believed; key the window by the truth.
        let host = opened.shell_host.clone();
        let token = new_token();
        let stale = {
            let mut inner = napplets.inner.lock().unwrap();
            let stale = inner.windows.remove(&host).map(|w| w.sessions());
            inner.sessions.insert(
                token.clone(),
                SessionRef {
                    id: opened.session_id.clone(),
                    host: host.clone(),
                },
            );
            inner.windows.insert(
                host.clone(),
                WindowEntry {
                    pointer: pointer.to_string(),
                    pending: Some(token),
                    live: None,
                },
            );
            stale.unwrap_or_default()
        };
        napplets.close_sessions(&stale);

        let url: tauri::Url = format!("http://{host}:{GATEWAY_PORT}/")
            .parse()
            .map_err(|e| format!("bad napplet URL for {host:?}: {e}"))?;
        let title = match (title.is_empty(), opened.title.as_deref()) {
            (false, _) => title.to_string(),
            (true, Some(t)) if !t.is_empty() => t.to_string(),
            _ => "napplet".to_string(),
        };
        // The shell never navigates and the napplet's frame never leaves: a
        // navigation away and back, or into an nsite host, would put other
        // content inside the origin the capability channel is scoped to
        // (`NappletWebViewClient.shouldOverrideUrlLoading`). Refused at the
        // webview, before any request is made.
        let own_host = host.clone();
        let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
            .title(title)
            .inner_size(1000.0, 760.0)
            .min_inner_size(360.0, 360.0)
            .on_navigation(move |url| {
                // WebKitGTK reports the napplet iframe's own `about:srcdoc`
                // load here too; that is the sandbox doing its job, not a
                // navigation to refuse.
                let stays = (url.scheme() == "about" && matches!(url.path(), "srcdoc" | "blank"))
                    || (url.scheme() == "http"
                        && url
                            .host_str()
                            .is_some_and(|h| h.eq_ignore_ascii_case(&own_host))
                        && url.port() == Some(GATEWAY_PORT)
                        && matches!(url.path(), "/" | "/index.html"));
                if !stays {
                    tracing::warn!(%url, "napplet window tried to navigate away; refused");
                }
                stays
            })
            .build()
            .map_err(|e| e.to_string())?;

        // Drop the sessions with the window. Rust ignores every later frame
        // for them, so a leaked page cannot keep a capability session alive.
        let app_for_close = app.clone();
        let host_for_close = host.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(napplets) = app_for_close.try_state::<Arc<Napplets>>() {
                    napplets.window_closed(&host_for_close);
                }
            }
        });
        Ok(())
    }

    fn window_closed(&self, host: &str) {
        let gone = {
            let mut inner = self.inner.lock().unwrap();
            inner.windows.remove(host).map(|w| w.sessions())
        };
        self.close_sessions(&gone.unwrap_or_default());
    }

    fn close_sessions(&self, tokens: &[String]) {
        if tokens.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        for token in tokens {
            if let Some(session) = inner.sessions.remove(token) {
                self.host.close(&session.id);
            }
        }
    }

    /// Whether `host` is a shell origin this gateway should answer for at all.
    pub fn is_shell_host(host: &str) -> bool {
        myco_napplet_runtime::is_shell_host(host)
    }

    /// Every request for a shell host: the page itself, or the channel.
    pub async fn serve(app: AppHandle, host: String, req: Request) -> Response {
        let Some(napplets) = app.try_state::<Arc<Napplets>>().map(|s| Arc::clone(&s)) else {
            return plain(503, "napplets are unavailable");
        };
        let path = req.uri().path().to_string();
        if let Some(rest) = path.strip_prefix(CHANNEL_PREFIX) {
            return napplets.channel(&host, rest, req).await;
        }
        napplets.serve_shell(app, &host, req).await
    }

    /// The shell page for one window, with this load's session prelude.
    ///
    /// Serves the shell and nothing else. The napplet's own bytes never pass
    /// through here — they are pushed over the channel and assigned to
    /// `srcdoc`. Serving them at this origin would make them reachable by
    /// URL, and anything navigating there would run the napplet as the shell.
    async fn serve_shell(&self, app: AppHandle, host: &str, req: Request) -> Response {
        if req.method() != axum::http::Method::GET {
            return plain(405, "Method Not Allowed");
        }
        if !matches!(req.uri().path(), "/" | "/index.html") {
            return plain(404, "Not Found");
        }
        // The shell page is a main-frame document, never a subframe's: the
        // napplet's iframe navigating itself here would otherwise be handed
        // the trusted shell page. WebKit says which it is; when it does not,
        // the `frame-ancestors 'none'` policy on the response still refuses
        // to render it framed.
        if let Some(dest) = header(&req, "sec-fetch-dest") {
            if dest != "document" {
                tracing::warn!(host, dest, "subframe request for the shell page; refused");
                return plain(403, "Forbidden");
            }
        }

        let pointer = {
            let inner = self.inner.lock().unwrap();
            let Some(entry) = inner.windows.get(host) else {
                // No window was opened for this origin: a stray navigation,
                // not a napplet. The Apps tab is the door.
                return plain(404, "no napplet window is open for this origin");
            };
            entry.pointer.clone()
        };

        // The first load adopts the session `open_window` resolved; a reload
        // (the `relaunch` frame after a grant change) opens its own. Either
        // way the session the page ran before is closed: one window, one
        // session.
        let pending = self
            .inner
            .lock()
            .unwrap()
            .windows
            .get_mut(host)
            .and_then(|w| w.pending.take());
        let token = match pending {
            Some(token) => token,
            None => {
                let app = app.clone();
                let target = pointer.clone();
                // Blocks on the resolve; never on a runtime worker.
                let opened = tokio::task::spawn_blocking(move || open_session(&app, &target))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                match opened {
                    Ok(opened) => {
                        let token = new_token();
                        self.inner.lock().unwrap().sessions.insert(
                            token.clone(),
                            SessionRef {
                                id: opened.session_id,
                                host: host.to_string(),
                            },
                        );
                        token
                    }
                    Err(e) => {
                        tracing::warn!(pointer, error = %e, "napplet did not reopen");
                        return plain(502, &format!("Couldn't open this app: {e}"));
                    }
                }
            }
        };
        let previous = {
            let mut inner = self.inner.lock().unwrap();
            match inner.windows.get_mut(host) {
                Some(entry) => entry.live.replace(token.clone()),
                None => None,
            }
        };
        if let Some(prev) = previous {
            self.close_sessions(&[prev]);
        }

        let page = format!("{}{}", prelude(&token), myco_napplet_runtime::shell_page());
        Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            // The shell is compiled into the binary and changes with the
            // app; a stale one would be a stale capability channel.
            .header(CACHE_CONTROL, "no-store")
            .header("Content-Security-Policy", "frame-ancestors 'none'")
            .header("X-Frame-Options", "DENY")
            .body(Body::from(page))
            .unwrap_or_else(|_| plain(500, "response build failed"))
    }

    /// `POST <prefix><token>/frame` carries one frame from the shell and
    /// answers with the frames to send back; `GET <prefix><token>/next`
    /// long-polls for runtime-initiated frames.
    async fn channel(&self, host: &str, rest: &str, req: Request) -> Response {
        let Some((token, op)) = rest.split_once('/') else {
            return plain(404, "Not Found");
        };
        // The token is the capability; it is also tied to the origin it was
        // minted for, so a session cannot be driven from another host. A
        // cross-site caller (the napplet's opaque-origin frame, were its CSP
        // ever to let a request out) is refused on the browser's word.
        let session = self.inner.lock().unwrap().sessions.get(token).cloned();
        let session = match session {
            Some(s) if s.host == host => s.id,
            _ => return plain(410, "session is closed"),
        };
        let session = session.as_str();
        if let Some(site) = header(&req, "sec-fetch-site") {
            if site != "same-origin" {
                tracing::warn!(
                    host,
                    site,
                    "napplet channel request from another site; refused"
                );
                return plain(403, "Forbidden");
            }
        }

        let frames = match (op, req.method().as_str()) {
            ("frame", "POST") => {
                let body = match axum::body::to_bytes(req.into_body(), 1 << 20).await {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(_) => return plain(413, "frame too large"),
                };
                self.host.frame(session, &body).await
            }
            ("next", "GET") => self.host.next_frames(session, DRAIN_WAIT).await,
            _ => return plain(404, "Not Found"),
        };
        let json = serde_json::to_string(&frames).unwrap_or_else(|_| "[]".to_string());
        Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .body(Body::from(json))
            .unwrap_or_else(|_| plain(500, "response build failed"))
    }
}

impl WindowEntry {
    fn sessions(self) -> Vec<String> {
        self.pending.into_iter().chain(self.live).collect()
    }
}

/// Resolve a napplet and open a session — `nappletOpen`'s shape: the runtime
/// lock is held only to gather the request, and the resolve (a relay read and
/// a blob read; a network round trip with a custom relay) runs with it
/// released, so a slow open never queues the reducer. Grants are read from
/// the Library on the core side, never taken from the caller.
fn open_session(app: &AppHandle, pointer: &str) -> Result<myco_core::OpenedNapplet, String> {
    let core = app.state::<Core>();
    let request = core
        .0
        .lock()
        .unwrap()
        .prepare_open_napplet(pointer)
        .map_err(|e| e.to_string())?;
    let (opened, widened) = request.run().map_err(|e| e.to_string())?;
    if widened {
        core.0.lock().unwrap().note_library_changed();
    }
    Ok(opened)
}

/// A channel token: 24 random bytes, unpadded URL-safe base64 — unguessable
/// by anything else on the loopback, which is what makes it a capability.
fn new_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("os rng");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The channel object the shell page reads as `window.mycoNappletRuntime`,
/// defined before the shell's own script runs.
///
/// Frames run one at a time, in order, until the handshake has answered
/// (`shell.init` seen in a reply) — `shell.ready` must land before the first
/// capability call or that call is refused as "not established" — and
/// overlap freely after it, so a query waiting on a slow relay does not hold
/// up the publish behind it. Same rule as `NappletActivity`'s frame loop.
/// A `relaunch` frame reloads the page: new session, fresh handshake, the
/// napplet's startup calls made over under the grants as they now stand.
fn prelude(token: &str) -> String {
    let base = serde_json::to_string(&format!("{CHANNEL_PREFIX}{token}/")).unwrap_or_default();
    format!(
        r#"<script>
(function () {{
  'use strict';
  var base = {base};
  var runtime = {{ onmessage: null }};
  var established = false;
  var queue = Promise.resolve();
  var stopped = false;
  function deliver(frames) {{
    for (var i = 0; i < frames.length; i++) {{
      var f = frames[i];
      if (!f) continue;
      if (f.channel === 'relaunch') {{ stopped = true; location.reload(); return; }}
      if (f.channel === 'napplet' && f.message && f.message.type === 'shell.init') established = true;
      if (typeof runtime.onmessage === 'function') runtime.onmessage({{ data: JSON.stringify(f) }});
    }}
  }}
  function send(body) {{
    return fetch(base + 'frame', {{ method: 'POST', headers: {{ 'content-type': 'application/json' }}, body: body }})
      .then(function (r) {{ return r.ok ? r.json() : []; }})
      .then(deliver)
      .catch(function () {{}});
  }}
  runtime.postMessage = function (body) {{
    body = String(body);
    if (established) send(body);
    else queue = queue.then(function () {{ return send(body); }});
  }};
  function drain() {{
    if (stopped) return;
    fetch(base + 'next')
      .then(function (r) {{ if (r.status === 410) {{ stopped = true; return []; }} return r.ok ? r.json() : []; }})
      .then(function (frames) {{ deliver(frames); if (!stopped) drain(); }})
      .catch(function () {{ if (!stopped) setTimeout(drain, 1000); }});
  }}
  Object.defineProperty(window, 'mycoNappletRuntime', {{ value: runtime, writable: false, configurable: false }});
  drain();
}})();
</script>
"#
    )
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Tauri window labels allow `[a-zA-Z0-9-/:_]`; shell hosts are bech32 plus
/// a d-tag, so anything else maps to `-` (same rule as nsite windows).
fn window_label(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("napplet-{safe}")
}

fn plain(status: u16, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message.to_string()))
        .unwrap()
}
