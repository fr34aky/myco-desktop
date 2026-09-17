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
  to systemd. Two things the daemon's own defaults get wrong for Myco, both
  operator settings rather than code: its `fips0` firewall baseline
  (`/etc/fips/fips.nft`, loaded by `fips-firewall.service`) is default-deny
  for anything a peer initiates, which is exactly what a pair request, a
  relay pull and a Blossom fetch are — `desktop/packaging/myco.nft` is the
  `/etc/fips/fips.d/` drop-in that opens 4870/4873/24243, and without it
  pairing looks like nothing happening while the daemon's drop counter
  climbs. And LAN rendezvous (`node.rendezvous.lan.enabled`) is off by
  default, so a same-Wi-Fi phone never finds the daemon; the phone still
  dials a *scoped* daemon advert, but a scoped daemon never dials the
  phone's unscoped one, so leave `scope` unset.
- **Embedded mode** (fallback for machines without a daemon): a fips node in
  process, as on Android, but with `TunPolicy::SystemTun` (fips creates and
  configures `fips0` itself — requires `CAP_NET_ADMIN`), BLE via fips's own
  BlueZ backend (no platform radio code at all), and both UDP lanes on. The
  node starts with the app. The one-time `sudo desktop/packaging/myco-setup
  <binary>` grants the capability (file capabilities die with the inode —
  re-run after rebuilds) and installs a systemd-resolved drop-in routing
  `~fips` lookups to the embedded responder, which binds the fixed
  `[::1]:5354` in this mode (elsewhere port 0 — nothing external needs it).
  Without the capability the node starts **TUN-less**: BLE/UDP still carry
  pairing and sync, but nothing on the machine routes `fd00::/8` or resolves
  `.fips`; Settings shows the exact remediation line. The local
  `reference/fips` checkout already carries the BlueZ PSM advertise/learn
  patches embedded BLE↔Android interop needs (build.md §4, patch 3).
- **Same-Wi-Fi lane.** A phone or desktop on the same LAN should be reached
  over UDP, not BLE (tens of kB/s and a flapping link vs. a LAN). The
  rendezvous is mDNS `_fips._udp` with the npub in TXT, the advert fips
  itself defines (`reference/fips` `src/mdns`); the phone both browses and
  publishes it (`ApRadio`). Embedded mode turns on fips's own LAN rendezvous,
  **unscoped**, so it advertises and dials. Daemon mode inherits whatever
  `/etc/fips/fips.yaml` says: the daemon typically advertises with a
  `scope`, which the phone's browser ignores-by-not-filtering, so the phone
  dials the daemon and the session lands on UDP either way — but a scoped
  daemon never dials the phone's unscoped advert itself. Identity is never
  taken from the advert; the Noise handshake authenticates the peer.

The two **cannot coexist** on one host: one `fd00::/8` route, one BLE PSM
(a second L2CAP bind fails and the transport marks itself Failed), and fips's
system-TUN creation deletes an existing `fips0`. Startup probes the system
control socket; the automatic choice can be forced (`MYCO_BACKEND` or a
`backend = "…"` line in `~/.config/myco/desktop.toml`), but forcing embedded
mode while a daemon answers is refused with a dialog. A daemon starting *after* an embedded node is
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
- File share: the hotspot share's HTTP server/consent-gate/outbox design
  with a mode toggle — **mesh only** (default: bound to the mesh ULA,
  advertised as the `.fips` name, Myco devices only; needs the share ports
  in the fips firewall drop-in) or **this network** (LAN bind, any browser
  on the Wi-Fi — the phone-hotspot audience). No AP either way.
- Paired file transfer (the core's encrypted mesh transfer, upstream #34):
  the shell only picks files (`share_file_with_peer`, "Send file…" on a
  Circle row) and publishes a finished receive — the poll loop moves every
  `completed` + `publishPending` row from the core's private `received/`
  staging dir to `~/Downloads/Myco` and forgets it, the MediaStore step on
  the phone. Incoming offers prompt through the in-shell modal queue; live
  and failed rows sit in a "File transfers" card on Circle.
- `myco://` deep links via the desktop-file scheme handler +
  single-instance plugin.
- Dev tab peer rows show every path fips holds to a peer (lane, state,
  min RTT, samples, ETX, score; `*` active, `b` backup) and the lane
  summary `aware [ble]` on line 2 — the phone's multi-path peer view. The
  desktop crate builds `myco-core` with `fips-multipath`, so BLE is a
  backup-role path as on Android and `multipath_core` is true.

## Napplets

The Apps grid holds two kinds of app. **Nsites** (kind 15128/35128) render
as full-page web apps with *direct* access to the local relay
(`ws://localhost:4870`) and Blossom. **Napplets** (NIP-5D, kind
5129/15129/35129; `docs/design/napplet/napplet-runtime.md`) are programs
Myco *hosts*: a sandboxed iframe inside a trusted shell page, with no
network of its own, reaching everything through capabilities the core
implements on its behalf. Fetching, verification, install review, grants,
every capability and the relaunch-on-grant-change all live in `myco-core`
and its runtime crate — the desktop, like the phone, is a pipe
(`desktop/src-tauri/src/napplets.rs`, the `NappletActivity` port).

- **Window.** One chrome-less window per napplet at
  `http://<label>.napplet.localhost:4880/`, served by the same loopback
  gateway as nsites but answered by the napplet host, never by the nsite
  gateway (and no nsite is served at a shell origin). The window is created
  only after the resolve succeeded — a napplet that fails verification gets
  no session and no window, and the reason lands on the Apps tab in words.
  Navigation is pinned to the shell URL (plus the iframe's own
  `about:srcdoc`): the shell never navigates, the napplet's frame never
  leaves.
- **Capability channel.** Android injects `mycoNappletRuntime` with
  `addWebMessageListener`, scoped to the shell origin. wry has no per-origin
  injection, and Tauri IPC is deliberately withheld from nsite and napplet
  windows (they are plain web pages), so the channel is loopback HTTP on
  the shell's own origin: the gateway prefixes the shell page with a prelude
  that defines the runtime object over `POST /__myco/napplet/<token>/frame`
  (one frame in, the replies out — `NappletHost::frame`) and a long poll on
  `GET …/next` (`next_frames`, 20 s). The token is random, minted per page
  load, tied to the origin it was minted for, and is the capability: the
  napplet's iframe has an opaque origin and a `connect-src 'none'` CSP and
  never sees it. The core's own session id (a counter) never leaves the
  process. Frames run one at a time until `shell.init` has answered, then
  overlap, exactly as the Activity's frame loop orders them.
- **Sessions.** One per page load. The first load adopts the session the
  open resolved; a `relaunch` frame (a grant changed on the sheet) reloads
  the page, which opens a fresh session and closes the previous one; closing
  the window closes its session. The shell page carries
  `frame-ancestors 'none'` and refuses a `Sec-Fetch-Dest` other than
  `document`, so it cannot be framed into another origin.
- **Shell UI.** Napplet tiles sit in the Apps grid with the duck badge and
  dim when their bytes are not on this device; the context menu adds
  *Manage permissions* (live switches per domain, in the install sheet's
  words) and *Reload app*. Install review is an overlay driven by
  `state.nappletReview`, the only place a grant is written; Discover
  suggests the same napplets as the phone; Settings › App reach holds the
  NAP-MESH hop caps. An `naddr` pasted into *Add*, a `myco://share` carrying
  `napplet`, and the `myco://napplet/<pointer>` launcher link all route to
  `fetch_napplet` (with the sharer as holder for a share) — never to a
  silent install.

## Ports

4870 (relay, mesh + loopback), 24243 (Blossom), the auth port — as on
Android; 4880 (nsite gateway **and** napplet shells + their capability
channel, loopback only) is desktop-new.

## Delivery

Implemented as a sequence of single-purpose PRs: core config refactor (this
page's `RuntimeConfig`/`MeshBackend` seam, no behavior change), host content
plane, daemon backend, desktop scaffold, gateway + nsite windows, Circle +
pairing + deep links, Discover/Settings/Dev, LAN file share, embedded mode.
Embedded-mode BLE interop with Android rides the BlueZ PSM
advertise/learn patches the local `reference/fips` checkout already carries
(see `docs/how-to/build.md` §4, patch 3).
