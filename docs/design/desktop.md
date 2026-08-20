# Myco Desktop

A Linux-first desktop app mirroring the Android feature set: the Apps grid,
nsites in their own chrome-less windows, Discover, Circle + pairing, Settings,
the Dev diagnostics, a LAN port of the file share, and `myco://` share links.
One Tauri v2 binary linking `myco-core` as an rlib — there is no FFI layer;
the desktop shell holds a `Mutex<AppRuntime>` exactly the way the JNI handle
does and drives `dispatch`/`state_json` directly.

## Why this is mostly a shell

`AppRuntime` already builds and runs on Linux; JNI is a thin string wrapper
over it. Nearly every Android screen is a pure render of `AppState` — the
desktop work is a new shell plus lifting the `#[cfg(target_os = "android")]`
gates that keep the content servers and the peer tick from running on host.
The Android-only surface that needs *replacement* rather than porting: the
nsite WebView (→ webview windows over a loopback gateway server), NFC pairing
(→ QR display + paste), the VpnService TUN (→ backend-dependent, below), and
the local-only hotspot (→ plain LAN file sharing).

## Mesh backends

The desktop runs in one of two modes, selected at startup (`MeshBackend` in
`myco-core`):

- **Daemon mode** (default when a system fips daemon is detected): no
  in-process node. `ControlClient` points at `/run/fips/control.sock`
  (`control_client::SYSTEM_SOCKET_PATH`; access gated by the `fips` group);
  the existing 8s tick (peer cache, connected-peers feed, site retries,
  keepwarm) runs against it unchanged. The relay (:4870), auth service, and
  Blossom (:24243) bind as on Android; inbound mesh traffic arrives over the
  daemon's TUN, and `CircleGate`'s source-IP checks work unchanged. Outbound
  content rides the daemon's TUN and `.fips` DNS via the system resolver —
  zero code. `StartNode`/`StopNode` become a hint: the mesh lifecycle belongs
  to systemd.
- **Embedded mode** (fallback for machines without a daemon): a fips node in
  process, as on Android, but with `TunPolicy::SystemTun` (fips creates and
  configures `fips0` itself — requires `CAP_NET_ADMIN`), BLE via fips's own
  BlueZ backend (no platform radio code at all), and both UDP lanes on.

The two **cannot coexist** on one host: one `fd00::/8` route, one BLE PSM
(a second L2CAP bind fails and the transport marks itself Failed), and fips's
system-TUN creation deletes an existing `fips0`. Startup probes the system
control socket; if the user forces embedded mode while a daemon answers, the
app refuses with an explanation. A daemon starting *after* an embedded node is
up is a documented limitation, not defended against.

Per-backend data dirs (`~/.local/share/myco/{daemon,embedded}/`) keep the two
identities' stores apart.

### Identity in daemon mode

Mesh reachability is bound to the daemon's npub — its ULA and `.fips` name —
so the content identity must be that npub, and the content layer must sign
pair/gossip/manifest events with its nsec. `/etc/fips/fips.key` is a bare
bech32 nsec, the same format `identity_store` persists. The supported setup is
a one-time privileged step:

```
sudo chgrp fips /etc/fips/fips.key && sudo chmod 0640 /etc/fips/fips.key
```

The `fips` group already gates the control socket (mutating commands
included), so this is not a materially new trust grant — but it does make the
identity machine-scoped and shared by all `fips`-group users. Alternative
(manual): point `fips.yaml` at a user-owned key. The long-term fix is a
signing command on the daemon's control API (upstream fips work, TBD).

If the key is unreadable the app comes up in a degraded read-only daemon mode:
browsing already-synced sites works, pairing/gossip/publishing are disabled,
and `AppState.error` carries the exact remediation line.

## Nsite serving

A loopback-only HTTP server on `127.0.0.1:4880` (fixed — origin stability is
load-bearing for nsite `localStorage`), routing on the `Host` header:
`http://<host>.localhost:4880/<path>` → the gateway (`Content::gateway_get`),
preserving status, headers and Range/206 exactly as `NsiteActivity` does. This
is chosen over a Tauri custom protocol because it is byte-faithful to
Android's `<host>.localhost` origin model, keeps the page's
`ws://localhost:4870` relay access as ordinary loopback traffic (no
custom-scheme CSP/origin risk in webkit2gtk), and is curl-testable. Startup
verifies `probe.localhost` resolves to loopback and errors clearly if not.

## Shell

- Tauri main process: `Mutex<AppRuntime>`, commands `dispatch`/`get_state`, a
  1 Hz poll thread (required — `poll_pending_start` rides it) emitting a
  `state` event when `rev` advances; the poll loop also hosts the ported
  pair-secret auto-accept.
- Shell window: five tabs, vanilla HTML/JS/CSS (AMOLED theme), no framework,
  no bundler.
- Ported pure logic: the single-use 30-min pair-secret ledger, pending deep
  links (30-day TTL), and the `myco://pair|share|app` payload codecs.
- Pairing v1 is QR **display** + paste; camera scanning is deferred.
- LAN file share: the hotspot share's HTTP server/consent-gate/outbox design
  bound to the LAN address — no AP; guests use the existing Wi-Fi and open
  the same themed page.
- `myco://` deep links via the desktop-file scheme handler +
  single-instance plugin.

## Ports

4870 (relay, mesh + loopback), 24243 (Blossom), the auth port — as on
Android; 4880 (nsite gateway, loopback only) is desktop-new.

## Delivery

Implemented as a sequence of single-purpose PRs: core config refactor (this
page's `RuntimeConfig`/`MeshBackend` seam, no behavior change), host content
plane, daemon backend, desktop scaffold, gateway + nsite windows, Circle +
pairing + deep links, Discover/Settings/Dev, LAN file share, embedded mode.
Embedded-mode BLE interop with Android additionally needs the BlueZ
PSM-advertising patch in the local `reference/fips` checkout (see
`docs/how-to/build.md` §4).
