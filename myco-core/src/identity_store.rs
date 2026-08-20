use std::path::Path;

const KEY_FILE: &str = "identity.nsec";

/// Load the persisted device `nsec` from `<data_dir>/identity.nsec`, generating
/// and persisting a fresh keypair on first launch.
///
/// On Android `data_dir` is the app-private `filesDir`, so the file is not
/// world-readable. The secret never leaves the Rust core (never crosses to the
/// WebView or JS).
pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(KEY_FILE);
    if path.exists() {
        let nsec = std::fs::read_to_string(&path)?.trim().to_string();
        if !nsec.is_empty() {
            return Ok(nsec);
        }
    }
    let id = fips::Identity::generate();
    let nsec = fips::encode_nsec(&id.keypair().secret_key());
    std::fs::write(&path, &nsec)?;
    Ok(nsec)
}

/// Read an externally-managed nsec — a system fips daemon's key file.
///
/// Never generates: the file's absence or unreadability is the caller's signal
/// to enter degraded (read-only) daemon mode, and writing into `/etc` is not
/// this process's place. The content is validated as a real secret key so a
/// permissions fix and a corrupt file produce different errors.
pub fn read_external(path: &Path) -> anyhow::Result<String> {
    let nsec = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?
        .trim()
        .to_string();
    fips::Identity::from_secret_str(&nsec)
        .map_err(|e| anyhow::anyhow!("{} is not a valid key: {e}", path.display()))?;
    Ok(nsec)
}
