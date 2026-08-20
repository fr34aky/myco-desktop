// Myco desktop shell — a hand-rolled render-on-event store: one JSON state
// snapshot in, one screen render out. The backend pushes a `state` event every
// second (peer rows and daemon status change without a rev bump, so no
// filtering); the active tab decides which renderer sees it.

"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let state = null;
let tab = "apps";

// ---------------------------------------------------------------- rendering

function esc(text) {
  const div = document.createElement("div");
  div.textContent = text ?? "";
  return div.innerHTML;
}

function render() {
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
  const renderer = { apps: renderApps, settings: renderSettings }[tab] || renderPlaceholder;
  screen.innerHTML = renderer();
}

function renderApps() {
  const sites = (state.sites || []).slice().sort((a, b) => (a.title || a.host).localeCompare(b.title || b.host));
  if (!sites.length) {
    return "<h1>Apps</h1><div class='empty'>No apps yet.</div>";
  }
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
      return `<div class="tile ${cls}" title="${esc(s.host)}">
        <div class="glyph">${esc(letter)}</div>
        <div class="title">${esc(title)}</div>
        ${sub}
      </div>`;
    })
    .join("");
  return `<h1>Apps</h1><div class="grid">${tiles}</div>`;
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

// ------------------------------------------------------------------ wiring

document.getElementById("tabs").addEventListener("click", (e) => {
  const button = e.target.closest("button[data-tab]");
  if (!button) return;
  tab = button.dataset.tab;
  for (const b of document.querySelectorAll("#tabs button")) {
    b.classList.toggle("active", b === button);
  }
  render();
});

listen("state", (event) => {
  state = JSON.parse(event.payload);
  render();
});

invoke("get_state").then((json) => {
  state = JSON.parse(json);
  render();
});

render();
