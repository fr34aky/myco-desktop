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
    settings: { slice: () => [state.identity, state.node, state.appVersion], render: renderSettings },
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

function renderSettings() {
  const id = state.identity || {};
  const node = state.node || {};
  return `<h1>Settings</h1>
    <div class="card">
      <h2>Mesh</h2>
      <div class="kv">
        <span class="k">status</span>
        <span class="v"><span class="dot ${node.running ? "on" : "off"}"></span>${esc(node.statusText)}</span>
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
    return;
  }
  const tile = e.target.closest(".tile[data-host]");
  if (tile) openApp(tile.dataset.host, tile.dataset.title);
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
