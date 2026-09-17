# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**Myco Desktop** — the Linux-first desktop edition of [Myco](https://github.com/Origami74/myco),
an offline-first, peer-to-peer client for **nsites** (static web apps published on
Nostr) shared over a [FIPS](https://github.com/k0sti/fips) mesh (BLE L2CAP + LAN/UDP),
with no internet and no app store.

This repo (`fr34aky/myco-desktop`) forks the upstream Android project and adds
`desktop/` — a Tauri v2 shell that links the same `myco-core` crate **directly as an
rlib, with no FFI**. The Android app (`android/`) is still in the tree and still built
by CI, but **only the desktop bundles are released** (`.github/workflows/release.yml`);
upstream owns the phone. Ported work is usually described as "Android parity" — when
in doubt about desktop behavior, the Android implementation is the spec.

## Commands

```bash
# --- Rust core (works anywhere; the default recipe) ---
just test                      # = cargo test (whole workspace)
cargo test -p myco-core <name> # single test / filter; skips the Tauri crate entirely
cargo fmt --all --check         # gating in CI
cargo clippy --all-targets -- -D warnings
just identity                   # host smoke check: prints device identity

# --- Desktop app ---
cargo build -p myco-desktop && ./target/debug/myco-desktop
sudo desktop/packaging/myco-setup ./target/debug/myco-desktop   # embedded mode only
cd desktop/src-tauri && cargo tauri build                       # .deb + AppImage

# --- Android (upstream surface; still builds here) ---
just build      # debug APK (Gradle cross-compiles the Rust via buildRustArm64)
just install    # build + adb install -r
cd android && ./gradlew testDebugUnitTest
```

Gotchas:

- `desktop/src-tauri` is a workspace member, so **any workspace-wide cargo command**
  (`cargo test`, `cargo build`, `cargo clippy --workspace`) needs the Tauri system
  libraries installed, or glib-sys/webkit build scripts fail:
  `sudo apt install -y pkg-config libdbus-1-dev libclang-dev libwebkit2gtk-4.1-dev
  libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev libsoup-3.0-dev`.
  Use `-p <crate>` to stay out of that.
- The `justfile` predates the desktop app: `just build`/`install`/`clean` are
  **Android** recipes. There is no `just` recipe for the desktop app.
- `myco-setup` grants `CAP_NET_ADMIN` on the **inode** — re-run it after every
  rebuild, or the embedded node comes up TUN-less.

Required before opening a PR (CONTRIBUTING.md): `cargo fmt --check`, `cargo build`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`, plus `./gradlew
assembleDebug` if you touched `android/`. Self-review against the 13-criteria
checklist in `PR-REVIEW.md`.

### The fips dependency

The workspace depends on `fips` as a **path dependency at `reference/fips`** — a
local, **gitignored** checkout. Nothing builds without it. CI clones
`github.com/jmcorgan/fips` branch `feat/multi-path-switchover` into place (see
`.github/workflows/ci.yml`; upstream Myco pins the same branch); the canonical
upstream is `github.com/k0sti/fips`. That branch carries the local patches Myco
needs: app-owned TUN, injectable `BleIo`, **per-peer PSM advertise/discover** (what
makes desktop BlueZ BLE interoperate with Android), a macOS `BleIo`, and multi-path
switchover (several links to one peer, probed standbys). The `fips-multipath` cargo
feature on `myco-core` turns the path-role configuration on; the desktop crate
enables it because its checkout is that branch. The Android Gradle build additionally
reads `MYCO_FIPS_REPO_PATH` to emit a `patch.crates-io` override; those builds
perturb `Cargo.lock`, so watch for a dirty lockfile afterwards. Details:
`docs/how-to/build.md` §4.

## Architecture

One Cargo workspace, six members. The five upstream crates build into the Android
`libmyco_core.so`; `desktop/src-tauri` is a plain binary on top of the same core.

- **`myco-core`** — the app crate (`lib` + `cdylib`). Owns device identity (one Nostr
  keypair, persisted on first launch), the mesh node, the Tokio multi-thread runtime
  (`runtime.rs`), the TUN packet bridge, `.fips` DNS interception, peer diagnostics,
  gossip, paired file transfer, and the napplet host (`napplet.rs`).
- **`nsite-deck`** — reusable, transport-agnostic nsite host: gateway (manifest → path
  → sha256 → serve), sync/import engine, propagator. Reaches the outside world only
  through four trait seams in `seams.rs`: `RelayBackend`, `BlobStore`, `PeerSource`,
  `FanoutSink`. It names no concrete relay, store, or radio — keep it that way.
- **`myco-napplet-runtime`** — NIP-5D napplet runtime: manifest parsing, resolution,
  the capability seams (identity, relay, outbox, mesh, resource), and the sandbox
  shell page (`assets/`). Transport-agnostic like `nsite-deck`.
- **`myco-relay`** — embedded NIP-01 relay implementing `RelayBackend` (ws on :4870).
  Durable events (manifests, replaceable kinds, notes) live in rust-nostr's LMDB
  store (`nostr-lmdb`, indexed NIP-01 queries, replaceable/addressable and NIP-09
  semantics applied by the database); events with a NIP-40 `expiration` (chat) are
  memory-only by design; deliberately no relay framework in front of it.
- **`myco-blossom`** — embedded Blossom blob store implementing `BlobStore` (http on
  :24243). Content-addressed by sha256; verifies hash on write (atomic temp+rename),
  trusts the name on read.
- **`desktop/src-tauri`** — the desktop shell (see below).

### RuntimeConfig — the platform seam

`AppRuntime::with_config(RuntimeConfig)` (`myco-core/src/runtime.rs`) is how a platform
declares what it wants; `RuntimeConfig::platform_default` reproduces Android's shape.
The fields that matter: `backend: MeshBackend` (`Embedded { ble, lan_udp, tun:
TunPolicy }` or `Daemon { control_socket }`), `start_content_servers`, and `data_dir`.
New platform differences belong in this config, **not** in fresh `cfg!(target_os)`
branches.

### The two mesh backends (desktop)

Chosen once at startup by `desktop/src-tauri/src/backend.rs`; they **cannot coexist**
(one `fd00::/8` route, one BLE PSM, and fips's system-TUN path deletes an existing
`fips0`):

- **Daemon** (default when `/run/fips/control.sock` answers) — no in-process node; the
  mesh lifecycle belongs to systemd, and `StartNode`/`StopNode` become hints. Content
  identity must be the daemon's npub, read from `/etc/fips/fips.key`; unreadable key →
  a degraded read-only mode with the remediation line in `AppState.error`.
- **Embedded** (fallback) — an in-process fips node like the phone, with
  `TunPolicy::SystemTun`, BLE via fips's own BlueZ backend, both UDP lanes, and mDNS
  (`_fips._udp`) so a same-Wi-Fi phone is dialled over UDP instead of BLE.

Each backend keeps its own data dir (`~/.local/share/myco/{daemon,embedded}/`) — the
identities differ and a store signed by one must never be continued under the other.
Overridable via `MYCO_BACKEND` or `~/.config/myco/desktop.toml`, but an override that
cannot work is refused with a dialog. Full rationale: `docs/design/desktop.md`.

### Desktop shell (`desktop/src-tauri/src/`)

- `main.rs` holds a `Mutex<AppRuntime>` (`Core`) — exactly what the JNI handle wraps —
  and exposes it through the `dispatch`/`get_state` Tauri commands (`commands.rs`),
  which take the same snake_case JSON actions the Kotlin client builds.
- `poll.rs` — a 1 Hz thread emitting the state snapshot as a `state` event. **Required,
  not decorative**: the reducer's `poll_pending_start` rides this cadence, and it also
  hosts pair auto-accept and the received-file publish.
- `gateway_http.rs` — loopback HTTP on the **fixed** `127.0.0.1:4880`, routing on the
  `Host` header: `http://<host>.localhost:4880/<path>` → `Content::gateway_get_page`;
  `*.napplet.localhost` hosts go to `napplets.rs` instead, never to the nsite gateway.
  Fixed port and `<host>.localhost` because that origin is the nsites' `localStorage`
  identity (byte-faithful to Android).
- `nsite_windows.rs` — one chrome-less webview window per nsite.
- `napplets.rs` — the `NappletActivity` port: one window per napplet at
  `http://<label>.napplet.localhost:4880/`, the shell page served by the gateway with
  a prelude that defines `mycoNappletRuntime` over loopback HTTP (`/__myco/napplet/
  <token>/frame` + a `/next` long poll). The random token is the capability; Tauri
  IPC is never granted to nsite or napplet windows. See `docs/design/desktop.md`.
- `pairing.rs` / `deeplinks.rs` — QR display + paste (no camera), the single-use 30-min
  pair-secret ledger, 30-day pending deep links, and the `myco://pair|share|app` codecs.
- `lanshare/` — the Android hotspot share minus the AP: mesh-only bind (default) or
  LAN bind, 90 s consent gate, uploads to `~/Downloads/Myco`.
- `filetransfer.rs` — the shell's half of the core's encrypted paired transfer: pick
  files, and move finished receives out of the private staging dir to `~/Downloads/Myco`.
- `desktop/ui/` — vanilla HTML/JS/CSS, **no framework and no bundler** (856-line
  `app.js`, render-on-event). Five tabs: Apps · Circle · Discover · Settings · Dev.
  `window.confirm/prompt` are unreliable in wry/WebKitGTK — use the in-shell modal
  queue; the webview console is invisible in a packaged build, so log via `ui_log`.

### Ports

4870 relay (loopback + mesh), 4873 auth (mesh-only; the one port an unpaired peer may
reach), 24243 Blossom, 4880 nsite gateway (desktop-only, loopback), `[::1]:5354`
`.fips` DNS responder (embedded mode). `docs/reference/ports.md`.

### Android surface (still in tree)

Kotlin ↔ Rust is a **JNI + JSON-over-strings Redux reducer**: `dispatch(actionJson) ->
stateJson` over an opaque `jlong`, with a monotonic `rev` (`myco-core/src/jni_abi.rs` ↔
`android/.../core/NativeCore.kt`). No UniFFI/bindgen. Actions in `action.rs`, the state
snapshot in `state.rs`. Constraints are LOCKED: **arm64-v8a only**, **minSdk 29**
(L2CAP CoC APIs), physical devices only.

`jni_abi.rs` compiles **only for Android**, so host `cargo test` never sees it — JNI
glue is verified only by the cargo-ndk build. Conversely many `myco-core` modules are
Android-only consumers marked `#[cfg_attr(not(target_os = "android"), allow(dead_code))]`;
host tests and the desktop drive `AppRuntime` directly. Rust talks to a running fips
node over its Unix-domain **control socket** (`control_client.rs`) — the only way to
read peer state or push a platform-discovered peer once the node's rx loop owns it.

## Testing philosophy

Host `cargo test` (~190 tests) uses in-memory mocks (`nsite-deck/src/testing.rs`:
`MemRelay`, `MemBlobs`) and is necessary but **not sufficient** for pairing/BLE/mesh
changes — those regress only on real hardware, usually a laptop plus a paired phone.
When core logic can't be host-tested, say so in the PR and describe the manual test.
See `docs/how-to/run-two-device-demo.md`.

## Conventions

- Single-trunk: branch off `main`, PR back into `main`, squash WIP commits.
- One logical change per PR; no drive-by reformatting or out-of-footprint cleanups.
- Design-affecting changes update the matching `docs/design/` page (desktop work →
  `docs/design/desktop.md`); user-visible changes update `README.md`; release notes go
  in `CHANGELOG.md` under `[Unreleased]`, written for users, not for the diff.
- Comments in this codebase explain *why* a thing is shaped the way it is (which
  constraint, which upstream bug, which Android behavior it mirrors). Match that.
- Many docs under `docs/design/` and `docs/how-to/` were written in forward-looking
  "proposal voice" before the code existed and still say **TBD / open** — notably
  `build.md`, which is Android-era. Where a doc and the tree disagree, **the tree is
  current** (justfile, Cargo.toml, `.github/workflows/`). `docs/design/core/concepts.md` is
  the glossary; start there for terminology (npub/node_addr, `.fips` vs `.nsite`,
  Pillars of Propagation).
