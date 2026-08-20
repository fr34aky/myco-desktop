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
  if (!button) return;
  tab = button.dataset.tab;
  for (const b of document.querySelectorAll("#tabs button")) {
    b.classList.toggle("active", b === button);
  }
  render(true);
});

listen("state", (event) => {
  state = JSON.parse(event.payload);
  render();
});

invoke("get_state").then((json) => {
  state = JSON.parse(json);
  render(true);
});

render();
