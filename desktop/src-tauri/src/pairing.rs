//! Pairing state the shell owns — the desktop port of the Android `share/`
//! helpers, wire-compatible with the phone:
//!
//! - `myco://pair/<b64url(json)>` payloads, `{v, npub, name, secret}` with
//!   unpadded URL-safe base64 (`NsiteShare.buildPairUri`).
//! - a **single-use** ledger of issued secrets, 30-minute TTL, persisted so a
//!   shown code survives a restart (`PairSecrets`).
//! - the device's memorable name: user override, else a color+name pair
//!   derived from the npub (`DeviceName`).
//!
//! The core stores none of this: the name rides outgoing pair events via
//! `set_device_name`, and the secret match is the shell-side auto-accept.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

const PAIR_PREFIX: &str = "myco://pair/";
const SHARE_PREFIX: &str = "myco://share/";
const APP_PREFIX: &str = "myco://app/";
const TTL_MS: u64 = 30 * 60 * 1000;

/// A decoded `myco://pair` (or the pairing half of a `myco://share`) payload.
#[derive(Debug, Clone)]
pub struct PairInfo {
    pub npub: String,
    pub name: String,
    pub secret: String,
}

/// What a scanned/pasted/deep-linked `myco://` URI asks for.
#[derive(Debug, Clone)]
pub enum Link {
    /// Pair with this device.
    Pair(PairInfo),
    /// Open this nsite and pair with its sharer.
    Share { nsite: String, pair: PairInfo },
    /// Open this nsite (public link, no secrets by design).
    App { host: String },
    /// Anything else: treated as a pasted nsite link, exactly like Android's
    /// `handleScannedText` fallback.
    Raw(String),
}

pub fn parse_link(text: &str) -> Link {
    let text = text.trim();
    if let Some(b64) = text.strip_prefix(PAIR_PREFIX) {
        if let Some(info) = decode_pair(b64) {
            return Link::Pair(info);
        }
    }
    if let Some(b64) = text.strip_prefix(SHARE_PREFIX) {
        if let Some((nsite, pair)) = decode_share(b64) {
            return Link::Share { nsite, pair };
        }
    }
    if let Some(rest) = text.strip_prefix(APP_PREFIX) {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_string();
        if !host.is_empty() {
            return Link::App { host };
        }
    }
    Link::Raw(text.to_string())
}

fn decode_json(b64: &str) -> Option<serde_json::Value> {
    let bytes = URL_SAFE_NO_PAD.decode(b64.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn decode_pair(b64: &str) -> Option<PairInfo> {
    let v = decode_json(b64)?;
    let s = |k: &str| v.get(k)?.as_str().map(str::to_string);
    let info = PairInfo {
        npub: s("npub")?,
        name: s("name").unwrap_or_default(),
        secret: s("secret").unwrap_or_default(),
    };
    (!info.npub.is_empty()).then_some(info)
}

fn decode_share(b64: &str) -> Option<(String, PairInfo)> {
    let v = decode_json(b64)?;
    let nsite = v.get("nsite")?.as_str()?.to_string();
    let pair = decode_pair(b64)?;
    (!nsite.is_empty()).then_some((nsite, pair))
}

/// Mint a pairing secret: 24 random bytes, unpadded URL-safe base64 — the
/// same shape `NsiteShare.newPairSecret` mints on the phone.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("os rng");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The shell's pairing state: the issued-secret ledger and the device name,
/// both persisted in the backend's data dir.
pub struct Pairing {
    data_dir: PathBuf,
    /// The secret currently shown in "My code". Cleared when consumed, so the
    /// next payload request mints a fresh one — the rotation that makes a
    /// captured QR single-use.
    current: Mutex<Option<String>>,
}

impl Pairing {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            current: Mutex::new(None),
        }
    }

    fn ledger_path(&self) -> PathBuf {
        self.data_dir.join("pair-secrets.json")
    }

    fn name_path(&self) -> PathBuf {
        self.data_dir.join("device-name.txt")
    }

    fn load_ledger(&self) -> HashMap<String, u64> {
        std::fs::read_to_string(self.ledger_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_ledger(&self, map: &HashMap<String, u64>) {
        if let Ok(json) = serde_json::to_string(map) {
            let _ = std::fs::write(self.ledger_path(), json);
        }
    }

    /// The current `myco://pair/…` URI, minting and recording a secret if none
    /// is live.
    pub fn pair_uri(&self, npub: &str, name: &str) -> String {
        let mut current = self.current.lock().unwrap();
        let secret = current.get_or_insert_with(|| {
            let secret = new_secret();
            let now = now_ms();
            let mut map = self.load_ledger();
            map.retain(|_, at| now.saturating_sub(*at) <= TTL_MS);
            map.insert(secret.clone(), now);
            self.save_ledger(&map);
            secret
        });
        let json = serde_json::json!({
            "v": 1,
            "npub": npub,
            "name": name,
            "secret": secret,
        });
        format!("{PAIR_PREFIX}{}", URL_SAFE_NO_PAD.encode(json.to_string()))
    }

    /// True exactly once per issued, unexpired secret — and rotates the shown
    /// code when the consumed secret is the current one.
    pub fn consume(&self, secret: &str) -> bool {
        if secret.is_empty() {
            return false;
        }
        let now = now_ms();
        let mut map = self.load_ledger();
        map.retain(|_, at| now.saturating_sub(*at) <= TTL_MS);
        let had = map.remove(secret).is_some();
        self.save_ledger(&map);
        if had {
            let mut current = self.current.lock().unwrap();
            if current.as_deref() == Some(secret) {
                *current = None;
            }
        }
        had
    }

    /// The device's memorable name: the persisted override, else one derived
    /// from the npub.
    pub fn device_name(&self, npub: &str) -> String {
        std::fs::read_to_string(self.name_path())
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| derived_name(npub))
    }

    pub fn set_device_name(&self, name: &str) {
        let _ = std::fs::write(self.name_path(), name.trim());
    }
}

/// A deterministic color+name pair from the npub — same idea as the phone's
/// `DeviceName.generated`, so one device always reads the same everywhere.
fn derived_name(npub: &str) -> String {
    const COLORS: [&str; 16] = [
        "green", "blue", "amber", "violet", "teal", "coral", "indigo", "rose", "olive", "cyan",
        "ruby", "slate", "jade", "plum", "mint", "navy",
    ];
    const NAMES: [&str; 16] = [
        "sammy", "rosa", "otto", "lena", "milo", "ada", "kai", "nova", "finn", "juno", "iris",
        "theo", "mika", "arlo", "nina", "cleo",
    ];
    let mut h: u64 = 1469598103934665603; // FNV-1a
    for b in npub.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!(
        "{} {}",
        COLORS[(h % 16) as usize],
        NAMES[((h >> 8) % 16) as usize]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_uri_round_trips_and_matches_the_android_shape() {
        let dir = std::env::temp_dir().join(format!("myco-pair-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pairing = Pairing::new(&dir);

        let uri = pairing.pair_uri("npub1abc", "green sammy");
        assert!(uri.starts_with(PAIR_PREFIX));
        let Link::Pair(info) = parse_link(&uri) else {
            panic!("must parse as a pair link: {uri}");
        };
        assert_eq!(info.npub, "npub1abc");
        assert_eq!(info.name, "green sammy");
        assert!(!info.secret.is_empty());

        // Single use: first consume wins, the second fails, and the shown code
        // rotates.
        assert!(pairing.consume(&info.secret));
        assert!(!pairing.consume(&info.secret));
        let next = pairing.pair_uri("npub1abc", "green sammy");
        let Link::Pair(rotated) = parse_link(&next) else {
            panic!()
        };
        assert_ne!(rotated.secret, info.secret, "the code must rotate");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_and_raw_links_parse() {
        match parse_link("myco://app/somehost/deep/path") {
            Link::App { host } => assert_eq!(host, "somehost"),
            other => panic!("{other:?}"),
        }
        match parse_link("  npub1xyz.nsite.lol  ") {
            Link::Raw(s) => assert_eq!(s, "npub1xyz.nsite.lol"),
            other => panic!("{other:?}"),
        }
    }
}
