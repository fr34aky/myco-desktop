//! LAN file sharing — the desktop port of the Android hotspot share.
//!
//! Same design, minus the access point, with a choice of audience ([`Mode`]):
//! **mesh-only** (the default) binds the mesh ULA and advertises the `.fips`
//! name, so only devices running Myco/fips can even connect — reaching the
//! laptop over fips also needs the share ports allowed in the fips firewall
//! drop-in (`/etc/fips/fips.d/`), since inbound on `fips0` is default-deny.
//! **This-network** is the opt-in equivalent of the phone's hotspot page:
//! wildcard bind, LAN IPv4 URL, any browser on the Wi-Fi.
//!
//! Everything else carries over from the phone: the guest *sends* through
//! the page and *receives* only what the owner pushes as an offer; every
//! transfer waits for the owner's OK (90 s timeout = deny); the shared-files
//! list is scoped to the session; uploads land in `~/Downloads/Myco`.

pub mod page;
pub mod server;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Undecided transfers are denied after this long, so an unattended laptop
/// never leaks a file. Matches the Android gate.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(90);

/// Who can reach the page.
///
/// [`Mode::Mesh`] is the default: the server binds the mesh ULA, the URL is
/// the `.fips` name, and only devices on the fips mesh — i.e. running
/// Myco/fips — can even connect. [`Mode::Lan`] is the opt-in "anyone nearby"
/// mode the hotspot share was: any browser on the local network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Mesh,
    Lan,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedFile {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OfferStatus {
    Waiting,
    Sent,
    Declined,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub status: OfferStatus,
    #[serde(skip)]
    pub path: PathBuf,
}

struct Running {
    url: String,
    url_svg: String,
    mode: Mode,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

#[derive(Default)]
struct Inner {
    running: Option<Running>,
    received: Vec<ReceivedFile>,
    offers: Vec<Offer>,
}

/// The whole share state, managed by Tauri and shared with the server.
pub struct LanShare {
    inner: Mutex<Inner>,
    next_id: AtomicU64,
    /// Consent waiters, keyed by request id; resolving sends the decision to
    /// the parked HTTP handler.
    waiters: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<bool>>>,
    app: OnceLock<AppHandle>,
    /// The core's tokio runtime, which the server rides.
    rt: OnceLock<tokio::runtime::Handle>,
    /// The mesh identity for [`Mode::Mesh`]: the ULA to bind and the `.fips`
    /// name to advertise. Unset in degraded daemon mode.
    mesh: OnceLock<(String, String)>,
}

impl Default for LanShare {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            next_id: AtomicU64::new(1),
            waiters: Mutex::new(HashMap::new()),
            app: OnceLock::new(),
            rt: OnceLock::new(),
            mesh: OnceLock::new(),
        }
    }
}

impl LanShare {
    pub fn attach(&self, app: AppHandle, rt: tokio::runtime::Handle) {
        let _ = self.app.set(app);
        self.attach_runtime(rt);
    }

    /// Runtime only — the test path (no Tauri app; events are skipped).
    pub fn attach_runtime(&self, rt: tokio::runtime::Handle) {
        let _ = self.rt.set(rt);
    }

    pub fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.rt.get().cloned()
    }

    /// The mesh ULA + `.fips` name mesh-mode binds and advertises.
    pub fn set_mesh(&self, ipv6: String, fips_host: String) {
        let _ = self.mesh.set((ipv6, fips_host));
    }

    pub fn mesh(&self) -> Option<(String, String)> {
        self.mesh.get().cloned()
    }

    fn emit_changed(&self) {
        if let Some(app) = self.app.get() {
            let _ = app.emit("lanshare", ());
        }
    }

    // ---- lifecycle ----------------------------------------------------

    pub fn set_running(
        &self,
        url: String,
        url_svg: String,
        shutdown: tokio::sync::oneshot::Sender<()>,
        mode: Mode,
    ) {
        self.inner.lock().unwrap().running = Some(Running {
            url,
            url_svg,
            mode,
            shutdown,
        });
        self.emit_changed();
    }

    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().running.is_some()
    }

    /// Stop the server and void the session: offers cleared, waiters denied.
    /// The received-files *list* clears with the session too (the files on
    /// disk of course remain), matching the phone.
    pub fn stop(&self) {
        let running = self.inner.lock().unwrap().running.take();
        if let Some(r) = running {
            let _ = r.shutdown.send(());
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.offers.clear();
            inner.received.clear();
        }
        for (_, tx) in self.waiters.lock().unwrap().drain() {
            let _ = tx.send(false);
        }
        self.emit_changed();
    }

    /// What the shell renders.
    pub fn status(&self) -> serde_json::Value {
        let inner = self.inner.lock().unwrap();
        serde_json::json!({
            "running": inner.running.is_some(),
            "url": inner.running.as_ref().map(|r| r.url.clone()),
            "urlSvg": inner.running.as_ref().map(|r| r.url_svg.clone()),
            "mode": inner.running.as_ref().map(|r| r.mode),
            "received": inner.received,
            "offers": inner.offers,
        })
    }

    // ---- the consent gate ---------------------------------------------

    /// Park until the owner decides (or the timeout denies). Server tasks only.
    pub async fn request_consent(
        &self,
        direction: &str,
        name: &str,
        size: u64,
        from: String,
    ) -> bool {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.waiters.lock().unwrap().insert(id, tx);
        match self.app.get() {
            Some(app) => {
                let sent = app.emit(
                    "transfer-request",
                    serde_json::json!({
                        "id": id,
                        "direction": direction,
                        "name": name,
                        "size": size,
                        "from": from,
                    }),
                );
                match sent {
                    Ok(()) => tracing::info!(id, name, "transfer-request emitted to the shell"),
                    Err(e) => tracing::warn!(id, name, error = %e, "transfer-request emit failed"),
                }
            }
            None => tracing::warn!(
                id,
                name,
                "no app handle attached — consent request is invisible"
            ),
        }
        let allowed = matches!(
            tokio::time::timeout(APPROVAL_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        self.waiters.lock().unwrap().remove(&id);
        allowed
    }

    /// The owner clicked Allow or Deny.
    pub fn decide(&self, id: u64, allow: bool) {
        if let Some(tx) = self.waiters.lock().unwrap().remove(&id) {
            let _ = tx.send(allow);
        }
    }

    // ---- received files ------------------------------------------------

    pub fn record_received(&self, name: String, size: u64) {
        self.inner
            .lock()
            .unwrap()
            .received
            .push(ReceivedFile { name, size });
        self.emit_changed();
    }

    pub fn received_list(&self) -> Vec<ReceivedFile> {
        self.inner.lock().unwrap().received.clone()
    }

    // ---- the outbox ----------------------------------------------------

    /// Offer picked files to the guest.
    pub fn add_offers(&self, paths: Vec<PathBuf>) {
        let mut inner = self.inner.lock().unwrap();
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file.bin".to_string());
            inner.offers.push(Offer {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                name,
                size: meta.len(),
                status: OfferStatus::Waiting,
                path,
            });
        }
        drop(inner);
        self.emit_changed();
    }

    /// What the guest's poll sees: only offers still waiting on them.
    pub fn waiting_offers(&self) -> Vec<Offer> {
        self.inner
            .lock()
            .unwrap()
            .offers
            .iter()
            .filter(|o| o.status == OfferStatus::Waiting)
            .cloned()
            .collect()
    }

    /// The guest accepted: mark SENT and hand back the path for streaming.
    /// No consent gate here — the owner's consent *was* offering the file.
    pub fn accept_offer(&self, id: u64) -> Option<Offer> {
        let mut inner = self.inner.lock().unwrap();
        let offer = inner
            .offers
            .iter_mut()
            .find(|o| o.id == id && o.status == OfferStatus::Waiting)?;
        offer.status = OfferStatus::Sent;
        let out = offer.clone();
        drop(inner);
        self.emit_changed();
        Some(out)
    }

    pub fn decline_offer(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(o) = inner.offers.iter_mut().find(|o| o.id == id) {
            o.status = OfferStatus::Declined;
        }
        drop(inner);
        self.emit_changed();
    }
}

/// Where uploads land: `~/Downloads/Myco`, created on first use.
pub fn downloads_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Myco")
}

/// A free name in the downloads dir: `name`, else `name (1)`, `name (2)`, …
pub fn free_path(name: &str) -> PathBuf {
    let dir = downloads_dir();
    let clean = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("upload.bin")
        .trim();
    let clean = if clean.is_empty() {
        "upload.bin"
    } else {
        clean
    };
    let candidate = dir.join(clean);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (clean.to_string(), String::new()),
    };
    for i in 1..1000 {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::Arc;

    use super::*;

    /// One tiny HTTP/1.1 client round-trip; enough for a loopback test.
    fn http(addr: &str, req: &str) -> String {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        out
    }

    /// The whole guest flow over real sockets: page, offer poll, accept
    /// (streams the file, marks SENT), and an upload the owner denies (403,
    /// nothing written).
    #[test]
    fn guest_flow_over_real_sockets() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let share = Arc::new(LanShare::default());
        share.attach_runtime(rt.handle().clone());

        let url = super::server::start(Arc::clone(&share), Mode::Lan).expect("server starts");
        // Talk to loopback regardless of which LAN ip the URL advertises.
        let port = url.trim_end_matches('/').rsplit(':').next().unwrap();
        let addr = format!("127.0.0.1:{port}");

        let page = http(
            &addr,
            "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(page.contains("200 OK"), "page must serve: {page:?}");
        assert!(page.contains("Share files with this computer"));

        // Offer a real file and let the "guest" poll and accept it.
        let dir = std::env::temp_dir().join(format!("myco-lanshare-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let offered = dir.join("hello.txt");
        std::fs::write(&offered, b"hello from the laptop").unwrap();
        share.add_offers(vec![offered]);

        let offers = http(
            &addr,
            "GET /offers HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(offers.contains("hello.txt"), "{offers:?}");
        let id = share.waiting_offers()[0].id;

        let body = http(
            &addr,
            &format!("GET /offer/{id} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
        );
        assert!(body.contains("hello from the laptop"), "{body:?}");
        assert!(body.contains("filename=\"hello.txt\""));
        assert!(
            share.waiting_offers().is_empty(),
            "accepted offer must leave waiting"
        );

        // An upload the owner denies: park the consent, deny it, expect 403
        // and no recorded file. The decider runs on a thread because the
        // request blocks until the decision.
        let share2 = Arc::clone(&share);
        let denier = std::thread::spawn(move || {
            // Wait for the waiter to appear, then deny it.
            for _ in 0..100 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let ids: Vec<u64> = share2.waiters.lock().unwrap().keys().copied().collect();
                if let Some(id) = ids.first() {
                    share2.decide(*id, false);
                    return;
                }
            }
            panic!("no consent request appeared");
        });
        let resp = http(
            &addr,
            "PUT /upload/secret.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nConnection: close\r\n\r\nnope!",
        );
        denier.join().unwrap();
        assert!(resp.contains("403"), "denied upload must 403: {resp:?}");
        assert!(share.received_list().is_empty());

        share.stop();
        let _ = std::fs::remove_dir_all(&dir);

        // Mesh mode, sequentially on the freed ports (parallel tests would
        // race the shared 8080..8083 pool): binds ONLY the mesh address, the
        // URL carries the `.fips` name, the mesh side answers, and the LAN
        // side gets connection refused — the property the mode exists for.
        // `::1` stands in for the ULA.
        let share = Arc::new(LanShare::default());
        share.attach_runtime(rt.handle().clone());
        share.set_mesh("::1".to_string(), "fakehost.fips".to_string());
        let url = super::server::start(Arc::clone(&share), Mode::Mesh).expect("mesh server starts");
        assert!(
            url.starts_with("http://fakehost.fips:"),
            "the URL must name the .fips address: {url}"
        );
        let port = url.trim_end_matches('/').rsplit(':').next().unwrap();
        let mesh_side = http(
            &format!("[::1]:{port}"),
            "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(
            mesh_side.contains("200 OK"),
            "mesh side must serve: {mesh_side:?}"
        );
        let lan_side = std::net::TcpStream::connect(format!("127.0.0.1:{port}"));
        assert!(
            lan_side.is_err(),
            "the LAN side must not connect in mesh mode"
        );
        share.stop();
    }
}
