//! The guest-facing HTTP server — the desktop's `FileShareServer`.
//!
//! Routes, matching the phone byte-for-byte where it matters:
//! - `GET /` — the themed page (uploads + session record + offer polling).
//! - `PUT /upload/<urlencoded-name>` — one raw body per file, name in the
//!   URL (the phone chose this over multipart deliberately; the multipart
//!   no-JS fallback is not ported — the consent flow needs JS anyway).
//! - `GET /offers` — offers still waiting, as JSON.
//! - `GET /offer/<id>` — the guest accepted: stream the file, mark SENT.
//! - `POST /offer/<id>/decline` — the guest declined.
//!
//! The owner is asked *before* an upload body is read — TCP backpressure
//! holds the guest's send while the request waits, and a denial answers with
//! `Connection: close` so the unread bytes can never garble a keep-alive.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use super::{page, LanShare, Mode};

const PORTS: [u16; 4] = [8080, 8081, 8082, 8083];

/// Bind and serve; returns the guest-facing URL. Runs on the given runtime.
///
/// [`Mode::Mesh`] (the default) binds the **mesh ULA only** — never a LAN
/// address, never wildcard — so the page is physically unreachable except
/// through `fips0`, and the URL names the `.fips` address (name, not
/// literal: resolving it is what warms the route on the guest's side). A
/// guest therefore has to be running Myco/fips.
///
/// [`Mode::Lan`] is the opt-in "anyone nearby" mode the hotspot share was:
/// wildcard bind, LAN IPv4 in the URL, any browser on the Wi-Fi. The mesh
/// side stays closed there in practice — the fips firewall default-denies
/// inbound on `fips0` unless the operator allowed these ports.
pub fn start(share: Arc<LanShare>, mode: Mode) -> Result<String, String> {
    let rt = share.runtime().ok_or("server runtime not attached")?;

    let (bind_addr, host): (std::net::IpAddr, String) = match mode {
        Mode::Mesh => {
            let (ipv6, fips_host) = share
                .mesh()
                .ok_or("no mesh identity — mesh-only sharing needs the fips address")?;
            let addr: std::net::Ipv6Addr = ipv6
                .parse()
                .map_err(|e| format!("mesh address {ipv6:?}: {e}"))?;
            (addr.into(), fips_host)
        }
        Mode::Lan => {
            let ip = lan_ip().ok_or("no LAN address found — is Wi-Fi/Ethernet up?")?;
            (
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                ip.to_string(),
            )
        }
    };

    let (listener, port) = rt.block_on(async {
        for port in PORTS {
            if let Ok(l) = tokio::net::TcpListener::bind((bind_addr, port)).await {
                return Ok((l, port));
            }
        }
        Err(format!(
            "could not bind {bind_addr} on any of {PORTS:?}{}",
            if mode == Mode::Mesh {
                " — is the mesh (fips0) up?"
            } else {
                ""
            }
        ))
    })?;

    let url = format!("http://{host}:{port}/");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let app = axum::Router::new()
        .route("/", get(page_handler))
        .route("/upload/{name}", put(upload))
        .route("/offers", get(offers))
        .route("/offer/{id}", get(send_offer))
        .route("/offer/{id}/decline", post(decline_offer))
        .with_state(share.clone());

    rt.spawn(async move {
        let serve = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        if let Err(e) = serve.await {
            tracing::warn!(error = %e, "lan share server exited");
        }
    });

    let svg = qrcode::QrCode::new(url.as_bytes())
        .map(|qr| {
            qr.render()
                .min_dimensions(220, 220)
                .dark_color(qrcode::render::svg::Color("#000000"))
                .light_color(qrcode::render::svg::Color("#ffffff"))
                .build()
        })
        .unwrap_or_default();
    share.set_running(url.clone(), svg, shutdown_tx, mode);
    Ok(url)
}

/// A LAN address a guest can type: prefer private IPv4, else any non-loopback.
fn lan_ip() -> Option<IpAddr> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    let v4 = |a: &if_addrs::Interface| match a.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(ip),
        _ => None,
    };
    addrs
        .iter()
        .filter_map(v4)
        .find(|ip| ip.is_private())
        .or_else(|| addrs.iter().filter_map(v4).next())
        .map(IpAddr::V4)
}

async fn page_handler(State(share): State<Arc<LanShare>>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        page::render(&share.received_list()),
    )
        .into_response()
}

async fn upload(
    State(share): State<Arc<LanShare>>,
    Path(name): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request<Body>,
) -> Response {
    let name = urlencoding_decode(&name);
    let Some(len) = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return (StatusCode::BAD_REQUEST, "length required\n").into_response();
    };

    // Owner first, body second (see module doc).
    let allowed = share
        .request_consent("upload", &name, len, peer.ip().to_string())
        .await;
    tracing::info!(name, len, %peer, allowed, "lan share upload");
    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONNECTION, "close")],
            "The owner did not approve this transfer.\n",
        )
            .into_response();
    }

    let path = super::free_path(&name);
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let Ok(mut file) = tokio::fs::File::create(&path).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed\n").into_response();
    };
    let mut stream = req.into_body().into_data_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            let _ = tokio::fs::remove_file(&path).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed\n").into_response();
        };
        if file.write_all(&chunk).await.is_err() {
            let _ = tokio::fs::remove_file(&path).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed\n").into_response();
        }
        written += chunk.len() as u64;
    }
    let _ = file.flush().await;
    share.record_received(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name),
        written,
    );
    (StatusCode::OK, "ok\n").into_response()
}

async fn offers(State(share): State<Arc<LanShare>>) -> Response {
    let offers: Vec<_> = share
        .waiting_offers()
        .iter()
        .map(|o| serde_json::json!({"id": o.id, "name": o.name, "size": o.size}))
        .collect();
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        serde_json::json!({ "offers": offers }).to_string(),
    )
        .into_response()
}

async fn send_offer(State(share): State<Arc<LanShare>>, Path(id): Path<u64>) -> Response {
    let Some(offer) = share.accept_offer(id) else {
        return (StatusCode::NOT_FOUND, "gone\n").into_response();
    };
    let Ok(file) = tokio::fs::File::open(&offer.path).await else {
        return (StatusCode::NOT_FOUND, "gone\n").into_response();
    };
    tracing::info!(name = offer.name, "lan share offer accepted by the guest");
    let stream = tokio_util::io::ReaderStream::new(file);
    // RFC 5987 filename* so non-ASCII names survive; quoted fallback too.
    let encoded: String = urlencoding_encode(&offer.name);
    let disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        offer.name.replace('"', "_"),
        encoded
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, offer.size)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "failed\n").into_response())
}

async fn decline_offer(State(share): State<Arc<LanShare>>, Path(id): Path<u64>) -> Response {
    share.decline_offer(id);
    (StatusCode::OK, "ok\n").into_response()
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn urlencoding_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
