//! The nsite gateway over loopback HTTP.
//!
//! `http://<host>.localhost:4880/<path>` → `Content::gateway_get_page`,
//! preserving status, headers and Range/206 exactly as the Android WebView
//! interceptor does. A loopback server rather than a Tauri custom protocol:
//! it is byte-faithful to Android's `<host>.localhost` origin model (per-site
//! storage isolation for free), keeps the page's `ws://localhost:4870` relay
//! access ordinary same-machine traffic, and is curl-testable
//! (docs/design/desktop.md).
//!
//! The port is fixed: the origin is the nsites' `localStorage` identity, so it
//! must be stable across restarts.
//!
//! Napplet shell hosts (`<label>.napplet.localhost`) are answered by
//! `napplets` instead — the shell page and its capability channel — and never
//! by the nsite gateway: an nsite must not be reachable at a shell origin,
//! nor a shell at an nsite one (`NappletWebViewClient` on the phone draws the
//! same line).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{HeaderName, HeaderValue, CONTENT_TYPE, HOST};
use axum::response::Response;
use myco_core::Content;
use tauri::AppHandle;

use crate::napplets::Napplets;

/// What every request is answered from: the content layer for nsites, the
/// app handle for napplet windows and their sessions.
#[derive(Clone)]
struct Gateway {
    content: Arc<Content>,
    app: AppHandle,
}

/// The gateway's loopback port. Desktop-new; see the ports section of the
/// design page.
pub const GATEWAY_PORT: u16 = 4880;

/// Bind and serve on the core's own tokio runtime. A bind failure is loud but
/// not fatal — the shell still runs; nsite windows would show connection
/// errors, and the port squatter is named in the log.
pub fn spawn(app: AppHandle, content: Arc<Content>, handle: tokio::runtime::Handle) {
    handle.spawn(async move {
        let addr = format!("127.0.0.1:{GATEWAY_PORT}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("myco-desktop: gateway bind {addr}: {e}");
                return;
            }
        };
        let router = axum::Router::new()
            .fallback(serve)
            .with_state(Gateway { content, app });
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("myco-desktop: gateway server exited: {e}");
        }
    });
}

/// Every request, any path: route on the `Host` header.
async fn serve(State(gateway): State<Gateway>, req: Request) -> Response {
    if let Some(shell) = shell_host(&req) {
        return Napplets::serve(gateway.app, shell, req).await;
    }
    serve_nsite(gateway.content, req).await
}

/// The napplet shell origin a request is for, lower-cased, if it is one.
/// Checked before the nsite suffix: `x.napplet.localhost` also ends in
/// `.localhost`.
fn shell_host(req: &Request) -> Option<String> {
    let host = req.headers().get(HOST)?.to_str().ok()?;
    let name = host.split(':').next()?;
    Napplets::is_shell_host(name).then(|| name.to_ascii_lowercase())
}

async fn serve_nsite(content: Arc<Content>, req: Request) -> Response {
    let Some(host) = nsite_host(&req) else {
        return plain(400, "expected Host: <site>.localhost");
    };

    let path = req.uri().path().to_string();
    let range = req
        .headers()
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // A passive probe (the shell's favicon fetch behind the Apps grid) must
    // never start a sync and pin the site; the shell marks those requests.
    let allow_sync = req.uri().query() != Some("nosync=1");

    let page = content
        .gateway_get_page(&host, &path, range.as_deref(), allow_sync)
        .await;

    let mut builder = Response::builder().status(page.status).header(
        CONTENT_TYPE,
        HeaderValue::from_str(&page.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    for (name, value) in &page.headers {
        if let (Ok(name), Ok(value)) = (
            name.parse::<HeaderName>(),
            HeaderValue::from_str(value.as_str()),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::from(page.body))
        .unwrap_or_else(|_| plain(500, "response build failed"))
}

/// `"abc.localhost:4880"` → `Some("abc.localhost")`; anything without an
/// nsite suffix → `None`. The suffix stays on: `resolve_host` inside the
/// gateway strips it itself (`.localhost` and `.nsite` alike) — stripping it
/// here too would hand it a bare label it cannot resolve.
fn nsite_host(req: &Request) -> Option<String> {
    let host = req.headers().get(HOST)?.to_str().ok()?;
    let name = host.split(':').next()?;
    let label = name
        .strip_suffix(".localhost")
        .or_else(|| name.strip_suffix(".nsite"))?;
    (!label.is_empty()).then(|| name.to_string())
}

fn plain(status: u16, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message.to_string()))
        .unwrap()
}
