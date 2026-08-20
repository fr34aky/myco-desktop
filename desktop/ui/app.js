// Myco desktop shell — a hand-rolled render-on-event store: one JSON state
// snapshot in, one screen render out. The backend pushes a `state` event every
// second; each tab re-renders only when the state slice it shows actually
// changed (so favicon <img>s aren't recreated — and re-fetched — every tick).

"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const GATEWAY = "localhost:4880";

let state = null;
let tab = "apps";
let lastRenderKey = "";
let deviceName = "";
let pairSvg = "";

function refreshPairing() {
  invoke("device_name").then((n) => {
    deviceName = n;
    render();
  });
  invoke("pair_payload")
    .then((p) => {
      pairSvg = p.svg;
      render();
    })
    .catch(() => {
      pairSvg = "";
      render();
    });
}

// ---------------------------------------------------------------- rendering

function esc(text) {
  const div = document.createElement("div");
  div.textContent = text ?? "";
  return div.innerHTML;
}

function render(force = false) {
  const banner = document.getElementById("error-banner");
  if (state && state.error) {
    banner.textContent = state.error;
    banner.classList.remove("hidden");
  } else {
    banner.classList.add("hidden");
  }

  const screen = document.getElementById("screen");
  if (!state) {
    screen.innerHTML = '<div class="empty">Starting…</div>';
    return;
  }
  const spec = {
    apps: { slice: () => [state.sites, state.updateCheck], render: renderApps },
    circle: {
      slice: () => [state.circle, state.reachableNpubs, state.blePeers,
                    state.outboundPairs, state.pendingPairRequests, deviceName, pairSvg],
      render: renderCircle,
    },
    discover: {
      slice: () => [state.discovered, state.library, state.sites],
      render: renderDiscover,
    },
    settings: {
      slice: () => [state.identity, state.node, state.appVersion, state.cache, state.offlineOnly],
      render: renderSettings,
    },
    dev: {
      slice: () => [state.peers, state.node, state.identity, state.speedtest, openPeers.size],
      render: renderDev,
    },
  }[tab] || { slice: () => [], render: renderPlaceholder };

  const key = tab + JSON.stringify(spec.slice());
  if (!force && key === lastRenderKey) return;
  lastRenderKey = key;
  screen.innerHTML = spec.render();
}

function renderApps() {
  const sites = (state.sites || [])
    .slice()
    .sort((a, b) => (a.title || a.host).localeCompare(b.title || b.host));
  const tiles = sites
    .map((s) => {
      const title = s.title || s.host;
      const letter = (title || "?").slice(0, 1).toUpperCase();
      let sub = "";
      let cls = "";
      if (s.state === "syncing") {
        cls = "syncing";
        sub = `<div class="sub">${s.filesPulled}/${s.filesTotal}</div>`;
      } else if (s.state === "unreachable" || s.state === "incomplete") {
        sub = `<div class="sub bad">${esc(s.state)}</div>`;
      } else if (s.updateAvailable) {
        sub = `<div class="sub">update ${s.updatePulled}/${s.updateTotal}</div>`;
      }
      return `<div class="tile ${cls}" data-host="${esc(s.host)}" data-title="${esc(title)}" title="${esc(s.host)}">
        <div class="glyph">
          <img src="http://${esc(s.host)}.${GATEWAY}/favicon.ico?nosync=1" alt=""
               onerror="this.remove()" />
          <span>${esc(letter)}</span>
        </div>
        <div class="title">${esc(title)}</div>
        ${sub}
      </div>`;
    })
    .join("");
  const add = `<div class="tile add" id="add-app"><div class="glyph">+</div><div class="title">Add</div></div>`;
  return `<h1>Apps</h1><div class="grid">${tiles}${add}</div>`;
}

function shortNpub(npub) {
  return npub && npub.length > 16 ? npub.slice(0, 10) + "…" + npub.slice(-4) : npub || "";
}

function renderCircle() {
  const circle = state.circle || [];
  const reachable = new Set(state.reachableNpubs || []);
  const inCircle = new Set(circle.map((c) => c.npub));
  const pending = state.pendingPairRequests || [];
  const invited = state.outboundPairs || [];
  const peers = (state.blePeers || []).filter(
    (p) => p.npub && !inCircle.has(p.npub) && !invited.some((i) => i.npub === p.npub)
  );

  const me = `<div class="card">
    <h2>This device</h2>
    <div class="me-row">
      <button id="rename" class="chip" title="Rename">${esc(deviceName) || "…"}</button>
      <span class="sub mono">${esc(shortNpub((state.identity || {}).ownNpub))}</span>
    </div>
    ${pairSvg
      ? `<div class="qr">${pairSvg}</div>
         <div class="sub center">Scan with your phone's Myco to pair — or paste their code below.</div>`
      : `<div class="empty">No pairing code (identity unavailable).</div>`}
    <div class="paste-row">
      <input id="pair-code" type="text" placeholder="Paste a myco:// code or nsite link" />
      <button id="pair-send">Go</button>
    </div>
  </div>`;

  const waiting = pending.length
    ? `<div class="card"><h2>Waiting to join</h2>${pending
        .map(
          (r) => `<div class="row">
            <span class="grow">${esc(r.name) || "unnamed"} <span class="sub mono">${esc(shortNpub(r.npub))}</span></span>
            <button data-act="accept" data-npub="${esc(r.npub)}" data-name="${esc(r.name)}">Accept</button>
            <button data-act="decline" data-npub="${esc(r.npub)}" class="ghost">Ignore</button>
          </div>`
        )
        .join("")}
        <div class="sub">Verify the name with the other device before accepting.</div></div>`
    : "";

  const invitedCard = invited.length
    ? `<div class="card"><h2>Invited</h2>${invited
        .map(
          (i) => `<div class="row">
            <span class="grow">${esc(i.name) || "unnamed"} <span class="sub mono">${esc(shortNpub(i.npub))}</span></span>
            <span class="sub">waiting…</span>
            <button data-act="cancel" data-npub="${esc(i.npub)}" class="ghost">Cancel</button>
          </div>`
        )
        .join("")}</div>`
    : "";

  const members = `<div class="card"><h2>In your circle (${circle.length})</h2>${
    circle.length
      ? circle
          .map(
            (c) => `<div class="row">
              <span class="dot ${reachable.has(c.npub) ? "on" : "off"}"></span>
              <span class="grow">${esc(c.name) || "unnamed"} <span class="sub mono">${esc(shortNpub(c.npub))}</span></span>
              <button data-act="remove" data-npub="${esc(c.npub)}" data-name="${esc(c.name)}" class="ghost danger">Remove</button>
            </div>`
          )
          .join("")
      : '<div class="empty">Nobody yet — pair with your phone via the code above.</div>'
  }</div>`;

  const nearby = peers.length
    ? `<div class="card"><h2>Connected peers</h2>${peers
        .map(
          (p) => `<div class="row">
            <span class="dot ${p.connected ? "on" : "off"}"></span>
            <span class="grow sub mono">${esc(shortNpub(p.npub))}</span>
            <button data-act="invite" data-npub="${esc(p.npub)}">＋ Invite</button>
          </div>`
        )
        .join("")}</div>`
    : "";

  return `<h1>Circle</h1>${me}${waiting}${invitedCard}${members}${nearby}`;
}

// The trio the phone also suggests — public nsites that pull from the Circle
// if a peer holds them, else the public fallback.
const SUGGESTED_APPS = [
  { title: "bitchat", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivbitchat" },
  { title: "ICS", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivics" },
  { title: "Dumplings", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivdumplings" },
];

function discoverTile(host, title, sub, holder) {
  const letter = (title || "?").slice(0, 1).toUpperCase();
  return `<div class="tile" data-host="${esc(host)}" data-title="${esc(title)}"
              ${holder ? `data-holder="${esc(holder)}"` : ""} title="${esc(host)}">
    <div class="glyph">
      <img src="http://${esc(host)}.${GATEWAY}/favicon.ico?nosync=1" alt="" onerror="this.remove()" />
      <span>${esc(letter)}</span>
    </div>
    <div class="title">${esc(title)}</div>
    ${sub ? `<div class="sub">${esc(sub)}</div>` : ""}
  </div>`;
}

function renderDiscover() {
  const suggested = SUGGESTED_APPS.map((s) => discoverTile(s.host, s.title, "", null)).join("");

  // Not news: a suggested app (offered above) or one already pinned — that
  // lives on the Apps tab. Same filter the phone applies.
  const offered = new Set(SUGGESTED_APPS.map((s) => s.host));
  for (const item of state.library || []) {
    if (item.pinned) offered.add(item.urlHost);
  }
  const around = (state.discovered || []).filter((d) => !offered.has(d.host));

  const aroundTiles = around.length
    ? around
        .map((d) =>
          discoverTile(d.host, d.title || d.host, `held by ${d.holderName || shortNpub(d.holderNpub)}`, d.holderNpub)
        )
        .join("")
    : `<div class="empty">Nothing new from your circle right now.</div>`;

  return `<h1>Discover <button id="discover-refresh" class="ghost-inline">Refresh</button></h1>
    <h2 class="section">Suggested</h2>
    <div class="grid">${suggested}</div>
    <h2 class="section">Around you</h2>
    ${around.length ? `<div class="grid">${aroundTiles}</div>` : aroundTiles}`;
}

function fmtBytes(n) {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return n.toFixed(i ? 1 : 0) + " " + units[i];
}

const STORAGE_CAP = 2_000_000_000;

function renderSettings() {
  const id = state.identity || {};
  const node = state.node || {};
  const cache = state.cache || {};
  const used = cache.usedBytes || 0;
  const pct = Math.min(100, (used / STORAGE_CAP) * 100);
  const daemonMode = (node.statusText || "").includes("daemon");
  return `<h1>Settings</h1>
    <div class="card">
      <h2>Mesh</h2>
      <div class="kv">
        <span class="k">status</span>
        <span class="v"><span class="dot ${node.running ? "on" : "off"}"></span>${esc(node.statusText)}</span>
      </div>
      ${daemonMode
        ? `<div class="sub" style="margin-top:.5rem">The mesh belongs to the system fips service — start or stop it with <span class="mono">systemctl</span>.</div>`
        : ""}
      <label class="toggle-row">
        <input type="checkbox" id="offline-only" ${state.offlineOnly ? "checked" : ""} />
        Mesh only — never use public internet relays as a fallback
      </label>
    </div>
    <div class="card">
      <h2>Storage</h2>
      <div class="gauge"><div class="gauge-fill" style="width:${pct}%"></div></div>
      <div class="kv" style="margin-top:.6rem">
        <span class="k">used</span><span class="v">${fmtBytes(used)} of ${fmtBytes(STORAGE_CAP)}</span>
        <span class="k">app files</span><span class="v">${cache.blobCount ?? "—"} blobs</span>
        <span class="k">events</span><span class="v">${cache.relayEvents ?? "—"}</span>
      </div>
      <div class="row" style="margin-top:.7rem">
        <button data-act="wipe-cache" class="ghost">Delete cache</button>
        <button data-act="wipe-stores" class="ghost danger">Delete all data, including apps</button>
      </div>
    </div>
    <div class="card">
      <h2>Identity</h2>
      <div class="kv">
        <span class="k">npub</span><span class="v">${esc(id.ownNpub) || "—"}</span>
        <span class="k">.fips address</span><span class="v">${esc(id.fipsAddr) || "—"}</span>
        <span class="k">mesh IPv6</span><span class="v">${esc(id.fipsIpv6) || "—"}</span>
        <span class="k">MTU</span><span class="v">${id.fipsMtu || "—"}</span>
      </div>
    </div>
    <div class="card">
      <h2>About</h2>
      <div class="kv"><span class="k">Myco</span><span class="v">${esc(state.appVersion)}</span></div>
    </div>`;
}

// ------------------------------------------------------------------ dev tab

const openPeers = new Set();

const PEER_DOT = {
  connected: "on",
  "reachable-via-relay": "on",
  "seen-unidentified": "warn",
  "paired-offline": "idle",
  unreachable: "off",
};

function agoText(ms) {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 10) return "now";
  if (s < 120) return `${s}s ago`;
  return `${Math.round(s / 60)}m ago`;
}

function renderDev() {
  const id = state.identity || {};
  const node = state.node || {};
  const peers = state.peers || [];
  const st = state.speedtest || {};

  const nodeCard = `<div class="card"><h2>Node &amp; fips</h2>
    <div class="kv">
      <span class="k">status</span>
      <span class="v"><span class="dot ${node.running ? "on" : "off"}"></span>${esc(node.statusText)}</span>
      <span class="k">node addr</span><span class="v mono">${esc(id.nodeAddrHex) || "—"}</span>
      <span class="k">mesh IPv6</span><span class="v mono">${esc(id.fipsIpv6) || "—"}</span>
    </div></div>`;

  const rows = peers
    .map((p) => {
      const key = p.key || p.npub;
      const label = p.name || shortNpub(p.npub) || p.bleAddr || "—";
      const attempts = (p.attempts || [])
        .slice(-20)
        .map((a) => {
          const t = new Date(a.atMs || 0).toTimeString().slice(0, 8);
          return `<div class="mono sub">${t} ${esc(a.role)} ${a.discoveryMs ?? ""}ms ${esc(a.outcome)}</div>`;
        })
        .join("");
      const speed =
        st.peerNpub === p.npub
          ? st.running
            ? `<span class="sub">testing… ${fmtBytes(st.bytes || 0)}</span>`
            : st.error
              ? `<span class="sub bad">${esc(st.error)}</span>`
              : st.upMbps || st.downMbps
                ? `<span class="sub">↑${(st.upMbps || 0).toFixed(1)} ↓${(st.downMbps || 0).toFixed(1)} Mbps</span>`
                : ""
          : "";
      return `<details data-peer="${esc(key)}" ${openPeers.has(key) ? "open" : ""}>
        <summary class="row">
          <span class="dot ${PEER_DOT[p.state] || "idle"}"></span>
          <span class="grow">${esc(label)}
            <span class="sub">${esc(p.transport || "")} · ${esc(p.state)} · ${agoText(p.lastSeenMs)}</span></span>
          ${p.state === "connected" && p.npub
            ? `<button data-act="speedtest" data-npub="${esc(p.npub)}">Speedtest</button>`
            : ""}
          ${speed}
        </summary>
        <div class="forensics">
          <div class="kv">
            <span class="k">npub</span><span class="v mono">${esc(p.npub) || "—"}</span>
            <span class="k">pair state</span><span class="v">${esc(p.pairState) || "—"} ${p.inCircle ? "· in circle" : ""}</span>
            <span class="k">role</span><span class="v">${esc(p.role) || "—"}</span>
            <span class="k">rssi / psm</span><span class="v">${p.rssi ?? "—"} / ${p.psm || "—"}</span>
            <span class="k">send drops</span><span class="v">${p.sendDrops ?? 0}</span>
          </div>
          ${attempts ? `<div style="margin-top:.4rem">${attempts}</div>` : ""}
        </div>
      </details>`;
    })
    .join("");

  return `<h1>Dev</h1>${nodeCard}
    <div class="card"><h2>Peers (${peers.length})</h2>
      ${rows || '<div class="empty">No peers observed.</div>'}
    </div>`;
}

function renderPlaceholder() {
  const label = tab.charAt(0).toUpperCase() + tab.slice(1);
  return `<h1>${label}</h1><div class="empty">${label} arrives in a later PR.</div>`;
}

// ------------------------------------------------------------ app actions

function dispatch(action) {
  return invoke("dispatch", { action: JSON.stringify(action) }).then((json) => {
    state = JSON.parse(json);
    render(true);
  });
}

function openApp(host, title) {
  // Android parity: opening a site *is* the install/sync trigger — the
  // reducer's open_nsite (re)starts a pull for a missing or stale site, so a
  // tile whose sync once failed heals on click instead of 404ing forever.
  dispatch({ type: "open_nsite", link: host });
  invoke("open_nsite_window", { host, title }).catch((e) => alert("Could not open: " + e));
}

// The context menu lives outside #screen so re-renders can't wipe it.
const menu = document.getElementById("menu");

function showMenu(x, y, host, title) {
  menu.innerHTML = `
    <button data-act="open">Open</button>
    <button data-act="update">Check for updates</button>
    <button data-act="remove" class="danger">Remove app</button>`;
  menu.style.left = Math.min(x, window.innerWidth - 200) + "px";
  menu.style.top = Math.min(y, window.innerHeight - 140) + "px";
  menu.classList.remove("hidden");
  menu.onclick = (e) => {
    const act = e.target.closest("button")?.dataset.act;
    menu.classList.add("hidden");
    if (act === "open") openApp(host, title);
    if (act === "update") dispatch({ type: "check_nsite_updates" });
    if (act === "remove" && confirm(`Remove ${title}? Its files stay cached until the cache is cleared.`)) {
      dispatch({ type: "forget_nsite", link: host });
    }
  };
}

document.addEventListener("click", (e) => {
  if (!e.target.closest("#menu")) menu.classList.add("hidden");
});

// ------------------------------------------------------------------ wiring

document.getElementById("screen").addEventListener("click", (e) => {
  if (e.target.closest("#add-app")) {
    const link = prompt("Paste an nsite link or host:");
    if (link && link.trim()) dispatch({ type: "open_nsite", link: link.trim() });
    return;
  }
  if (e.target.closest("#rename")) {
    const name = prompt("Device name (what peers see when pairing):", deviceName);
    if (name && name.trim()) {
      invoke("rename_device", { name: name.trim() }).then((json) => {
        state = JSON.parse(json);
        refreshPairing();
      });
    }
    return;
  }
  if (e.target.closest("#pair-send")) {
    const input = document.getElementById("pair-code");
    const text = (input.value || "").trim();
    if (text) {
      invoke("handle_link", { text });
      input.value = "";
    }
    return;
  }
  if (e.target.closest("#discover-refresh")) {
    dispatch({ type: "search_nsites" });
    return;
  }
  const act = e.target.closest("button[data-act]");
  if (act) {
    const { act: kind, npub, name } = act.dataset;
    if (kind === "accept") dispatch({ type: "accept_pair_request", npub, name: name || "" });
    if (kind === "decline") dispatch({ type: "decline_pair_request", npub });
    if (kind === "cancel") dispatch({ type: "cancel_pair_invite", npub });
    if (kind === "remove" && confirm(`Remove ${name || npub} from your circle?`)) {
      dispatch({ type: "remove_from_circle", npub });
    }
    if (kind === "invite") {
      invoke("invite_peer", { npub, name: "" }).then((json) => {
        state = JSON.parse(json);
        render(true);
      });
    }
    if (kind === "speedtest") dispatch({ type: "speedtest_peer", npub });
    if (kind === "wipe-cache" && confirm("Delete the cache? Files backing pinned apps are kept.")) {
      dispatch({ type: "wipe_cache" });
    }
    if (
      kind === "wipe-stores" &&
      confirm("Delete ALL data including apps? Identity and circle are kept. This cannot be undone.")
    ) {
      dispatch({ type: "wipe_stores" });
    }
    return;
  }
  const tile = e.target.closest(".tile[data-host]");
  if (tile) {
    const { host, title, holder } = tile.dataset;
    if (holder) {
      // Around-you: pull from the circle peer who holds it, then open.
      dispatch({ type: "open_nsite", link: host, holder });
      invoke("open_nsite_window", { host, title }).catch((err) => alert("Could not open: " + err));
    } else {
      openApp(host, title);
    }
  }
});

// Dev-tab forensics stay open across the 1 Hz re-render.
document.getElementById("screen").addEventListener(
  "toggle",
  (e) => {
    const details = e.target.closest("details[data-peer]");
    if (!details) return;
    if (details.open) openPeers.add(details.dataset.peer);
    else openPeers.delete(details.dataset.peer);
  },
  true
);

// The mesh-only toggle.
document.getElementById("screen").addEventListener("change", (e) => {
  if (e.target.id === "offline-only") {
    dispatch({ type: "set_offline_only", enabled: e.target.checked });
  }
});

document.getElementById("screen").addEventListener("contextmenu", (e) => {
  const tile = e.target.closest(".tile[data-host]");
  if (!tile) return;
  e.preventDefault();
  showMenu(e.clientX, e.clientY, tile.dataset.host, tile.dataset.title);
});

document.getElementById("tabs").addEventListener("click", (e) => {
  const button = e.target.closest("button[data-tab]");
  if (button) activateTab(button.dataset.tab);
});

function activateTab(name) {
  tab = name;
  for (const b of document.querySelectorAll("#tabs button")) {
    b.classList.toggle("active", b.dataset.tab === name);
  }
  if (name === "circle") refreshPairing();
  // Entering Discover asks the circle what it holds, like the phone.
  if (name === "discover") dispatch({ type: "search_nsites" });
  render(true);
}

listen("state", (event) => {
  state = JSON.parse(event.payload);
  render();
});

listen("pair-rotated", () => refreshPairing());

listen("goto", (event) => activateTab(event.payload));

invoke("get_state").then((json) => {
  state = JSON.parse(json);
  render(true);
});

refreshPairing();
render();
