//! Manual smoke check for daemon mode against this machine's system fips
//! daemon. Prints the state snapshot after a few ticks; expect the daemon's
//! npub in `identity`, `node.running: true`, and its peers in `blePeers`.
//!
//! Run: `cargo run -p myco-core --example daemon_status`
//! (needs a running `fips` service and a readable `/etc/fips/fips.key` —
//! see docs/design/desktop.md for the one-time permission step).

fn main() {
    let dir = std::env::temp_dir().join("myco-daemon-status-example");
    let mut rt = myco_core::AppRuntime::with_config(myco_core::RuntimeConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        app_version: "example".to_string(),
        backend: myco_core::MeshBackend::Daemon {
            control_socket: "/run/fips/control.sock".into(),
            key_file: "/etc/fips/fips.key".into(),
        },
        start_content_servers: false,
    });
    // Let the 8s status/peer tick answer at least once.
    std::thread::sleep(std::time::Duration::from_secs(2));
    println!("{}", rt.state_json());
}
