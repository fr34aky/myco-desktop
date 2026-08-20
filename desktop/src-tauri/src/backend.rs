//! Which mesh backend this machine gets.
//!
//! A system fips daemon and an embedded node cannot coexist (one `fd00::/8`
//! route, one BLE PSM, and fips's system-TUN path deletes an existing
//! `fips0`), so the choice is made once at startup: if the system control
//! socket answers `show_status`, the daemon owns the mesh and Myco runs
//! against it; otherwise Myco embeds its own node — BLE via fips's BlueZ
//! backend, both UDP lanes, and the system TUN when the process carries
//! `CAP_NET_ADMIN` (granted once by `desktop/packaging/myco-setup`; without
//! it the node runs TUN-less and Settings says how to fix that).
//!
//! The automatic choice can be overridden — `MYCO_BACKEND=daemon|embedded`,
//! or `backend = "daemon"` in `~/.config/myco/desktop.toml` — but never into
//! a broken shape: forcing embedded while a daemon answers (or daemon while
//! none does) is refused with an explanation instead of fought out over the
//! one mesh route. Each backend keeps its own data dir — the identities
//! differ, and a store signed by one must never be continued under the other.

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
    /// No daemon: a fips node in this process, as on Android.
    Embedded {
        /// Whether the process may create the system TUN (`CAP_NET_ADMIN`).
        /// Without it the node still runs — BLE and LAN UDP carry pairing and
        /// sync — but nothing on this machine can route to `fd00::/8` or
        /// resolve `.fips`, so mesh-mode file sharing and peer-served pages
        /// are off until `myco-setup` runs.
        system_tun: bool,
    },
}

impl Choice {
    fn embedded() -> Self {
        Choice::Embedded {
            system_tun: has_net_admin(),
        }
    }

    pub fn runtime_config(&self) -> RuntimeConfig {
        let (backend, dir_tag) = match self {
            Choice::Daemon => (
                MeshBackend::Daemon {
                    control_socket: PathBuf::from(myco_core::SYSTEM_SOCKET_PATH),
                    key_file: PathBuf::from(DAEMON_KEY_FILE),
                },
                "daemon",
            ),
            Choice::Embedded { system_tun } => (
                MeshBackend::Embedded {
                    ble: true,
                    lan_udp: true,
                    tun: if *system_tun {
                        TunPolicy::SystemTun
                    } else {
                        TunPolicy::Disabled
                    },
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

    /// What the Settings screen needs to explain this backend.
    pub fn info(&self) -> serde_json::Value {
        match self {
            Choice::Daemon => serde_json::json!({ "backend": "daemon", "tunLess": false }),
            Choice::Embedded { system_tun } => serde_json::json!({
                "backend": "embedded",
                "tunLess": !system_tun,
                "binary": std::env::current_exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "myco-desktop".into()),
            }),
        }
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Choice::Daemon => write!(f, "system fips daemon ({})", myco_core::SYSTEM_SOCKET_PATH),
            Choice::Embedded { system_tun: true } => write!(f, "embedded fips node (system TUN)"),
            Choice::Embedded { system_tun: false } => write!(
                f,
                "embedded fips node (TUN-less — no CAP_NET_ADMIN; run myco-setup)"
            ),
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

/// The backend override, if the user set one: `MYCO_BACKEND` wins, then a
/// `backend = "…"` line in `~/.config/myco/desktop.toml`. The file is read
/// by hand — one known key, quoted string, no nesting — deliberately not
/// worth a TOML dependency until a second setting exists.
fn forced() -> Option<String> {
    let value = std::env::var("MYCO_BACKEND").ok().or_else(|| {
        let path = dirs::config_dir()?.join("myco").join("desktop.toml");
        let text = std::fs::read_to_string(path).ok()?;
        text.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "backend").then(|| value.trim().trim_matches(['"', '\'']).to_string())
        })
    })?;
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty() && value != "auto").then_some(value)
}

/// True when the process may create and configure network devices — the
/// effective-capability bitmap in `/proc/self/status`, bit `CAP_NET_ADMIN`.
fn has_net_admin() -> bool {
    const CAP_NET_ADMIN: u32 = 12;
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let hex = status.lines().find_map(|l| l.strip_prefix("CapEff:"))?;
            u64::from_str_radix(hex.trim(), 16).ok()
        })
        .is_some_and(|caps| caps & (1 << CAP_NET_ADMIN) != 0)
}

/// Pick the backend; `Err` is a refusal to start, worded for a dialog.
pub fn detect() -> Result<Choice, String> {
    let probe = probe_daemon();
    match forced().as_deref() {
        None => Ok(match probe {
            Ok(()) => Choice::Daemon,
            Err(e) => {
                eprintln!("myco-desktop: no system daemon ({e}); using the embedded backend");
                Choice::embedded()
            }
        }),
        Some("daemon") => probe.map(|()| Choice::Daemon).map_err(|e| {
            format!(
                "The backend is forced to \"daemon\", but no system fips daemon answered ({e}).\n\n\
                 Start it with `systemctl start fips.service`, or remove the override \
                 (MYCO_BACKEND / the backend line in ~/.config/myco/desktop.toml)."
            )
        }),
        Some("embedded") => match probe {
            Ok(()) => Err(
                "The backend is forced to \"embedded\", but a system fips daemon is \
                 running — the two cannot share one host (one fd00::/8 route, one BLE PSM, and \
                 fips's TUN setup deletes an existing fips0).\n\n\
                 Stop the daemon with `systemctl stop fips.service`, or remove the override \
                 (MYCO_BACKEND / the backend line in ~/.config/myco/desktop.toml)."
                    .into(),
            ),
            Err(_) => Ok(Choice::embedded()),
        },
        Some(other) => Err(format!(
            "Unknown backend override {other:?} — expected \"daemon\", \"embedded\", or \"auto\" \
             (MYCO_BACKEND / the backend line in ~/.config/myco/desktop.toml)."
        )),
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
