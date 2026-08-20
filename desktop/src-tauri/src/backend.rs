//! Which mesh backend this machine gets.
//!
//! A system fips daemon and an embedded node cannot coexist (one `fd00::/8`
//! route, one BLE PSM, and fips's system-TUN path deletes an existing
//! `fips0`), so the choice is made once at startup: if the system control
//! socket answers `show_status`, the daemon owns the mesh and Myco runs
//! against it; otherwise Myco runs its own (for now transport-less) node.
//! Each backend keeps its own data dir — the identities differ, and a store
//! signed by one must never be continued under the other.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

use myco_core::{MeshBackend, RuntimeConfig, TunPolicy};

/// Where the system daemon keeps its identity key (a bare bech32 nsec). Myco
/// reads it to sign content as the daemon's npub — see docs/design/desktop.md
/// for the one-time permission step and the degraded mode without it.
const DAEMON_KEY_FILE: &str = "/etc/fips/fips.key";

/// How long the startup probe waits on the socket before concluding there is
/// no daemon. Local Unix-socket round-trip; a second is generous.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub enum Choice {
    /// A system fips daemon answered — run against it.
    Daemon,
    /// No daemon: an embedded node with no transports yet. Content browsing
    /// and the servers work; the packet plane arrives with embedded mode
    /// proper (TUN + BlueZ BLE, later PR).
    ContentOnly,
}

impl Choice {
    pub fn runtime_config(&self) -> RuntimeConfig {
        let (backend, dir_tag) = match self {
            Choice::Daemon => (
                MeshBackend::Daemon {
                    control_socket: PathBuf::from(myco_core::SYSTEM_SOCKET_PATH),
                    key_file: PathBuf::from(DAEMON_KEY_FILE),
                },
                "daemon",
            ),
            Choice::ContentOnly => (
                MeshBackend::Embedded {
                    ble: false,
                    lan_udp: false,
                    tun: TunPolicy::Disabled,
                },
                "embedded",
            ),
        };
        RuntimeConfig {
            data_dir: data_dir(dir_tag).to_string_lossy().into_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            backend,
            start_content_servers: true,
        }
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Choice::Daemon => write!(f, "system fips daemon ({})", myco_core::SYSTEM_SOCKET_PATH),
            Choice::ContentOnly => write!(f, "embedded (content only, no transports yet)"),
        }
    }
}

/// `~/.local/share/myco/<backend>/` — per-backend stores, never mixed.
fn data_dir(tag: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("myco")
        .join(tag)
}

/// Probe the system control socket with one `show_status` round-trip.
///
/// A connect alone is not enough — a stale socket file accepts nothing, and a
/// half-dead daemon that cannot answer its own status is not one to depend
/// on. Any failure means "no daemon" and the embedded fallback.
pub fn detect() -> Choice {
    match probe_daemon() {
        Ok(()) => Choice::Daemon,
        Err(e) => {
            eprintln!("myco-desktop: no system daemon ({e}); using the embedded backend");
            Choice::ContentOnly
        }
    }
}

fn probe_daemon() -> Result<(), String> {
    let stream = std::os::unix::net::UnixStream::connect(myco_core::SYSTEM_SOCKET_PATH)
        .map_err(|e| format!("connect {}: {e}", myco_core::SYSTEM_SOCKET_PATH))?;
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(PROBE_TIMEOUT)))
        .map_err(|e| format!("socket timeouts: {e}"))?;

    let mut writer = &stream;
    writer
        .write_all(b"{\"command\":\"show_status\"}\n")
        .map_err(|e| format!("write: {e}"))?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| format!("shutdown: {e}"))?;

    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| format!("read: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))?;
    match value.get("status").and_then(|s| s.as_str()) {
        Some("ok") => Ok(()),
        other => Err(format!("unexpected status {other:?}")),
    }
}
