// Myco desktop shell — a hand-rolled render-on-event store: one JSON state
// snapshot in, one screen render out. The backend pushes a `state` event every
// second; each tab re-renders only when the state slice it shows actually
// changed (so favicon <img>s aren't recreated — and re-fetched — every tick).

"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// The webview console is unreadable in a packaged wry window — errors and key
// diagnostics are forwarded to the app log instead (the `ui_log` command).
const uiLog = (m) => invoke("ui_log", { message: String(m) }).catch(() => {});
window.addEventListener("error", (e) =>
  uiLog(`error: ${e.message} @ ${e.filename}:${e.lineno}`)
);
window.addEventListener("unhandledrejection", (e) =>
  uiLog(`unhandled rejection: ${e.reason}`)
);

const GATEWAY = "localhost:4880";

let state = null;
let tab = "apps";
let lastRenderKey = "";

// ---- in-shell dialogs -----------------------------------------------------
// wry/WebKitGTK does not reliably implement window.confirm/prompt — a native
// confirm() can throw or return undefined, silently killing the caller. All
// questions go through this queue instead: one dialog at a time, oldest
// first, no dismiss-on-outside-tap (matching the phone's transfer dialog).

const modalQueue = [];
let modalActive = false;

function showNextModal() {
  const overlay = document.getElementById("modal-overlay");
  const box = document.getElementById("modal");
  const next = modalQueue.shift();
  if (!next) {
    modalActive = false;
    overlay.classList.add("hidden");
    return;
  }
  modalActive = true;
  const { text, html, input, yes, no, resolve } = next;
  box.innerHTML = `
    <p>${esc(text)}</p>
    ${html || ""}
    ${input !== undefined ? `<input id="modal-input" type="text" />` : ""}
    <div class="modal-actions">
      ${no ? `<button id="modal-no" class="ghost">${esc(no)}</button>` : ""}
      <button id="modal-yes">${esc(yes)}</button>
    </div>`;
  if (input !== undefined) {
    const field = document.getElementById("modal-input");
    field.value = input;
    setTimeout(() => field.focus(), 0);
  }
  overlay.classList.remove("hidden");
  const settle = (ok) => {
    const value = input !== undefined ? document.getElementById("modal-input").value : true;
    resolve(ok ? value : null);
    showNextModal();
  };
  document.getElementById("modal-yes").onclick = () => settle(true);
  const noBtn = document.getElementById("modal-no");
  if (noBtn) noBtn.onclick = () => settle(false);
}

function ask(text, { html, input, yes = "OK", no = "Cancel" } = {}) {
  return new Promise((resolve) => {
    modalQueue.push({ text, html, input, yes, no, resolve });
    if (!modalActive) showNextModal();
  });
}

const askConfirm = (text, yes = "OK") => ask(text, { yes }).then((v) => v !== null);
const askPrompt = (text, initial = "") => ask(text, { input: initial });
const askInfo = (text) => ask(text, { yes: "OK", no: null });
let deviceName = "";

// Which mesh backend this instance runs on (daemon / embedded, TUN-less?).
// Fetched once — the choice is made at startup and never changes.
let backendInfo = null;
invoke("backend_info").then((info) => {
  backendInfo = info;
  render(true);
});
let pairSvg = "";
let lanshare = null;
let lanshareMode = "mesh"; // the toggle's selection while stopped

function refreshLanshare() {
  invoke("lanshare_status").then((s) => {
    lanshare = s;
    render();
  });
}

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
  renderReview();
  const spec = {
    apps: {
      slice: () => [state.sites, state.updateCheck, state.library, state.nappletStatus],
      render: renderApps,
    },
    circle: {
      slice: () => [state.circle, state.reachableNpubs, state.blePeers,
                    state.outboundPairs, state.pendingPairRequests, state.fileTransfers,
                    deviceName, pairSvg, lanshare],
      render: renderCircle,
    },
    discover: {
      slice: () => [state.discovered, state.library, state.sites],
      render: renderDiscover,
    },
    settings: {
      slice: () => [state.identity, state.node, state.appVersion, state.cache, state.offlineOnly,
                    state.nappletMeshReach],
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

// nsites and napplets share one grid — they arrive the same way and open the
// same way from here; what differs is the badge and the menu behind it.
function installedNapplets() {
  return (state.library || []).filter((i) => i.kind === "napplet" && i.pinned);
}

function nappletTile(item) {
  const title = item.title || item.dTag || "napplet";
  const letter = (title || "N").slice(0, 1).toUpperCase();
  // Dimmed like an nsite that is not downloaded: the app is in the Library
  // but its bytes are not on this device (after "Delete cache", say).
  const status = (state.nappletStatus || []).find((s) => s.host === item.urlHost);
  const missing = status && status.state === "missing";
  return `<div class="tile ${missing ? "dim" : ""}" data-pointer="${esc(item.pointer)}"
              data-shell="${esc(item.urlHost)}" data-title="${esc(title)}" title="${esc(item.dTag || item.pointer)}">
    <div class="glyph"><span>${esc(letter)}</span><span class="badge" title="napplet">🦆</span></div>
    <div class="title">${esc(title)}</div>
    ${missing ? `<div class="sub bad">not on this device</div>` : ""}
  </div>`;
}

function renderApps() {
  const sites = (state.sites || [])
    .slice()
    .sort((a, b) => (a.title || a.host).localeCompare(b.title || b.host));
  const napplets = installedNapplets()
    .sort((a, b) => (a.title || a.dTag || "").localeCompare(b.title || b.dTag || ""))
    .map(nappletTile)
    .join("");
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
  return `<h1>Apps</h1><div class="grid">${tiles}${napplets}${add}</div>`;
}

// ---- napplets: install review and permissions (the phone's Apps-tab sheets)

// A NAP domain in words a person can act on. Unknown domains are shown
// verbatim rather than hidden: a napplet asking for something this build has
// never heard of is exactly what the user should see.
function capabilityWording(domain) {
  return {
    relay: ["Relays", "Read and post as you on your relays, without asking each time"],
    outbox: ["Outbox", "Post as you to your relays and to other people's, and read from theirs"],
    mesh: ["Mesh", "Send and receive data within your Circle, without the internet"],
    identity: ["Identity", "See your name and profile"],
    resource: ["Pictures & files", "Load pictures and files by their content hash"],
    storage: ["Storage", "Save things on this device"],
    intent: ["Other apps", "Open your other apps"],
    inc: ["App to app", "Talk to your other open apps"],
    notify: ["Notifications", "Send you notifications"],
    theme: ["Theme", "Match your colours"],
    link: ["Links", "Open links outside Myco"],
    config: ["Settings", "Have settings you can change"],
    shell: ["Start up", "Every app does this"],
  }[domain] || [domain, "Something this version of Myco doesn't know about"];
}

function capabilityRow(domain, { checkbox = false, on = true } = {}) {
  const [title, detail] = capabilityWording(domain);
  return `<label class="cap ${on ? "" : "off"}">
    ${checkbox ? `<input type="checkbox" data-grant="${esc(domain)}" ${on ? "checked" : ""} />` : ""}
    <span><div class="cap-title">${esc(title)}</div><div class="cap-detail">${esc(detail)}</div></span>
  </label>`;
}

// The install-review sheet: what a napplet is asking for, before it has it.
// This is the only place a grant is written — fetching stores bytes and
// grants nothing. Driven by state.nappletReview, so a fetch that finishes
// while another tab is up still asks, and a dismissed sheet stays dismissed.
let lastReviewKey = "";
function renderReview() {
  const overlay = document.getElementById("review-overlay");
  const review = state && state.nappletReview;
  const key = review ? JSON.stringify(review) : "";
  if (key === lastReviewKey) return;
  lastReviewKey = key;
  if (!review) {
    overlay.classList.add("hidden");
    return;
  }
  const box = document.getElementById("review");
  let body;
  if (review.loading) {
    body = `<h3>Fetching app…</h3>
      <div class="desc">Looking for it on ${review.holder ? "the sharer's device and " : ""}the relays.</div>
      <div class="modal-actions"><button id="review-dismiss" class="ghost">Cancel</button></div>`;
  } else if (review.error) {
    body = `<h3>Couldn't fetch this app</h3>
      <div class="desc">${esc(review.error)}</div>
      <div class="modal-actions">
        <button id="review-dismiss" class="ghost">Close</button>
        <button id="review-retry">Try again</button>
      </div>`;
  } else {
    const grants = (review.grants || []).map((d) => capabilityRow(d)).join("");
    body = `<h3>${esc(review.title) || "Untitled app"} <span class="sub">🦆 napplet</span></h3>
      ${review.description ? `<div class="desc">${esc(review.description)}</div>` : ""}
      <div class="sub" style="margin:.6rem 0 .2rem">This app will be able to:</div>
      ${grants || '<div class="sub">Nothing beyond starting up.</div>'}
      <div class="sub" style="margin-top:.6rem">You can switch any of these off later from the app's menu.</div>
      <div class="modal-actions">
        <button id="review-dismiss" class="ghost">Not now</button>
        <button id="review-install">Add to my apps</button>
      </div>`;
  }
  box.innerHTML = body;
  overlay.classList.remove("hidden");
  const dismiss = document.getElementById("review-dismiss");
  if (dismiss) dismiss.onclick = () => dispatch({ type: "dismiss_napplet_review" });
  const retry = document.getElementById("review-retry");
  if (retry) {
    retry.onclick = () =>
      dispatch({ type: "fetch_napplet", pointer: review.pointer, holder: review.holder || undefined });
  }
  const install = document.getElementById("review-install");
  if (install) {
    install.onclick = () =>
      dispatch({ type: "install_napplet", pointer: review.pointer, granted: review.grants || [] });
  }
}

// What a napplet may do, in the same words the install sheet used, with a
// switch for each. A change is live: an open window restarts under it.
function managePermissions(pointer, title) {
  const item = installedNapplets().find((i) => i.pointer === pointer);
  if (!item) return;
  const granted = new Set(item.granted || []);
  const rows = (state.nappletDomains || [])
    .map((d) => capabilityRow(d, { checkbox: true, on: granted.has(d) }))
    .join("");
  ask(`${title} may:`, { html: `<div data-permissions="${esc(pointer)}">${rows}</div>`, yes: "Done", no: null });
}

function openNapplet(item) {
  const title = item.title || item.dTag || "napplet";
  invoke("open_napplet_window", { host: item.urlHost, pointer: item.pointer, title }).catch((e) =>
    // Said out loud: a window that never appears reads as a click that did
    // not register, and hides that the app is gone.
    askInfo(`Couldn't open this app: ${e || "it isn't on this device"}`)
  );
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
              <button data-act="send-file" data-npub="${esc(c.npub)}" data-name="${esc(c.name)}" title="Send a file over the mesh">Send file…</button>
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

  return `<h1>Circle</h1>${me}${waiting}${invitedCard}${renderTransfers()}${members}${nearby}${renderLanshare()}`;
}

// ---- paired file transfer (the core's encrypted mesh transfer, Android #34)
// Statuses a transfer can still move on from; everything else is terminal.
const LIVE_TRANSFER = new Set(["offered", "waiting_user", "accepted", "ready", "downloading"]);

function transferLabel(t) {
  const peer = t.peerName || shortNpub(t.peerNpub) || "peer";
  const out = t.direction === "outgoing";
  switch (t.status) {
    case "offered": return `Waiting for ${peer} to accept`;
    case "waiting_user": return "Waiting for you to decide";
    case "accepted": return out ? "Preparing secure transfer" : `Waiting for ${peer}'s file`;
    case "ready": return `Sending securely to ${peer}`;
    case "downloading": return `Receiving from ${peer}`;
    case "completed": return "Done";
    case "denied": return t.error || "Declined";
    case "cancelled": return "Cancelled";
    case "failed": return t.error || "Transfer failed";
    default: return t.status;
  }
}

// Live rows get Cancel; terminal ones Dismiss. Mirrors the phone's Circle tab:
// a send waiting on an absent peer stays visible here until it resolves.
function renderTransfers() {
  const rows = (state.fileTransfers || []).filter(
    (t) => LIVE_TRANSFER.has(t.status) || ["failed", "denied", "cancelled"].includes(t.status)
  );
  if (!rows.length) return "";
  return `<div class="card"><h2>File transfers</h2>${rows
    .map((t) => {
      const live = LIVE_TRANSFER.has(t.status);
      const arrow = t.direction === "outgoing" ? "↑" : "↓";
      const decide =
        t.status === "waiting_user"
          ? `<button data-act="ft-accept" data-id="${esc(t.id)}">Accept</button>
             <button data-act="ft-decline" data-id="${esc(t.id)}" class="ghost">Decline</button>`
          : live
          ? `<button data-act="ft-cancel" data-id="${esc(t.id)}" class="ghost">Cancel</button>`
          : `<button data-act="ft-forget" data-id="${esc(t.id)}" class="ghost">Dismiss</button>`;
      return `<div class="row">
        <span class="grow">${arrow} ${esc(t.name)} <span class="sub">${t.size ? fmtBytes(t.size) : ""}</span>
          <div class="sub ${live ? "" : "danger"}">${esc(transferLabel(t))}</div></span>
        ${decide}
      </div>`;
    })
    .join("")}</div>`;
}

// Offers already put in front of the user, so a 1 Hz snapshot that still
// carries the row does not ask again.
const promptedOffers = new Set();

function promptIncomingOffers() {
  for (const t of state.fileTransfers || []) {
    if (t.direction !== "incoming" || t.status !== "waiting_user" || promptedOffers.has(t.id)) continue;
    promptedOffers.add(t.id);
    const from = t.peerName || shortNpub(t.peerNpub) || "A paired device";
    const size = t.size ? ` (${fmtBytes(t.size)})` : "";
    const html = t.mime ? `<div class="sub center mono">${esc(t.mime)}</div>` : "";
    uiLog(`file offer received: ${t.id} ${t.name} from ${from}`);
    ask(`${from} wants to send you "${t.name}"${size}. Save it to ~/Downloads/Myco?`, {
      html, yes: "Accept", no: "Decline",
    }).then((v) => {
      // The offer may have expired or been cancelled while the card was up;
      // the reducer ignores a decision for a row no longer waiting.
      dispatch({ type: v !== null ? "accept_file_transfer" : "decline_file_transfer", transferId: t.id });
    });
  }
}

function renderLanshare() {
  const ls = lanshare;
  if (!ls || !ls.running) {
    return `<div class="card"><h2>Share files</h2>
      <label class="toggle-row"><input type="radio" name="ls-mode" value="mesh" ${lanshareMode === "mesh" ? "checked" : ""} />
        Mesh only — reachable at your .fips address, Myco devices only</label>
      <label class="toggle-row"><input type="radio" name="ls-mode" value="lan" ${lanshareMode === "lan" ? "checked" : ""} />
        This network — anyone on your Wi-Fi can open the page</label>
      <div class="sub" style="margin-top:.5rem">Either way, every transfer waits for your OK here.</div>
      <div class="row" style="margin-top:.6rem"><button data-act="ls-start">Start sharing</button></div>
    </div>`;
  }
  const offers = (ls.offers || [])
    .map(
      (o) => `<div class="row"><span class="grow">${esc(o.name)}</span>
        <span class="sub">${o.status === "waiting" ? "waiting…" : o.status}</span></div>`
    )
    .join("");
  const received = (ls.received || [])
    .map((f) => `<div class="row"><span class="grow">${esc(f.name)}</span><span class="sub">received</span></div>`)
    .join("");
  const modeLine =
    ls.mode === "mesh"
      ? "Mesh only — open it on a device running Myco (your phone resolves .fips addresses)."
      : "This network — anyone on your Wi-Fi can open the page.";
  return `<div class="card"><h2>Share files</h2>
    <div class="kv"><span class="k">page</span><span class="v mono">${esc(ls.url)}</span></div>
    ${ls.urlSvg ? `<div class="qr" style="margin-top:.6rem">${ls.urlSvg}</div>` : ""}
    <div class="sub center">${esc(modeLine)}</div>
    ${offers ? `<h2 style="margin-top:.9rem">Sending</h2>${offers}` : ""}
    ${received ? `<h2 style="margin-top:.9rem">Received (in ~/Downloads/Myco)</h2>${received}` : ""}
    <div class="sub" style="margin-top:.6rem">To send straight to a paired phone's Myco app, use "Send file…" on its row above.</div>
    <div class="row" style="margin-top:.7rem">
      <button data-act="ls-send">Offer files on the page</button>
      <button data-act="ls-stop" class="ghost danger">Stop</button>
    </div>
  </div>`;
}

// The set the phone also suggests — public nsites that pull from the Circle
// if a peer holds them, else the public fallback.
const SUGGESTED_APPS = [
  { title: "bitchat", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivbitchat" },
  { title: "ICS", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivics" },
  { title: "Dumplings", host: "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivdumplings" },
];

// Napplet suggestions, keyed as the Library keys them (author + d-tag) so an
// installed one is recognised whatever pointer spelling it was added under.
// The pointer is an naddr, not `<npub>:<d>`: its relay hints ride inside it
// and are where the fetch looks first. Same list as the phone's Discover.
const SUGGESTED_NAPPLETS = [
  {
    title: "Mappy",
    pointer: "naddr1qqyx6ctswpkx2arnqgsqhtasevhkqty908ymemjgwuphelgrv33gf62p64ywy5ldum0as5srqsqqpzfe5a247a",
    authorNpub: "npub1pwhmpje0vqkg27wfhnhysacr0n7sxerzsn55r42guff7meklmpfqka6r38",
    dTag: "mapplets",
  },
  {
    title: "Minesweeper",
    pointer: "naddr1qq9k66twv4ehwet9wpjhyqg4waehxw309aex2mrp0yhxg6t5w3hjuur4vgpzqfngzhsvjggdlgeycm96x4emzjlwf8dyyzdfg4hefp89zpkdgz99qvzqqqyf8yzehfvw",
    authorNpub: "npub1ye5ptcxfyyxl5vjvdjar2ua3f0hynkjzpx552mu5snj3qmx5pzjscpknpr",
    dTag: "minesweeper",
  },
  {
    title: "DingDong",
    pointer: "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyt8wumn8ghj7un9d3shjtnswf5k6ctv9ehx2aqpp4mhxue69uhkummn9ekx7mqpz4mhxue69uhhyetvv9ujuerfw36x7tnsw43qqzryd9hxwer0denstp6v0k",
    authorNpub: "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr",
    dTag: "dingdong",
  },
];

function suggestedNappletTile(s) {
  const letter = s.title.slice(0, 1).toUpperCase();
  return `<div class="tile" data-fetch="${esc(s.pointer)}" data-title="${esc(s.title)}" title="${esc(s.dTag)}">
    <div class="glyph"><span>${esc(letter)}</span><span class="badge" title="napplet">🦆</span></div>
    <div class="title">${esc(s.title)}</div>
  </div>`;
}

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
  // A napplet already in the Library is on the Apps tab; it is not news here.
  // Nsite suggestions stay listed when installed (unchanged).
  const installed = installedNapplets();
  const napplets = SUGGESTED_NAPPLETS.filter(
    (s) => !installed.some((i) => i.authorNpub === s.authorNpub && i.dTag === s.dTag)
  )
    .map(suggestedNappletTile)
    .join("");
  const suggested = SUGGESTED_APPS.map((s) => discoverTile(s.host, s.title, "", null)).join("") + napplets;

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
  const reach = state.nappletMeshReach || { publishTtl: 0, publishMax: 0, subscribeTtl: 0, subscribeMax: 0 };
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
      ${backendInfo?.backend === "embedded"
        ? `<div class="kv"><span class="k">backend</span><span class="v">embedded fips node${backendInfo.tunLess ? " (no TUN)" : ""}</span></div>`
        : ""}
      ${backendInfo?.tunLess
        ? `<div class="banner" style="margin-top:.6rem">The node runs without its TUN — peers reach this device, but nothing here can open <span class="mono">.fips</span> pages or receive mesh file shares. One-time fix (repeat after every rebuild):<br/><span class="mono">sudo desktop/packaging/myco-setup ${esc(backendInfo.binary)}</span><br/>then restart Myco.</div>`
        : ""}
      <label class="toggle-row">
        <input type="checkbox" id="offline-only" ${state.offlineOnly ? "checked" : ""} />
        Mesh only — never use public internet relays as a fallback
      </label>
    </div>
    <div class="card">
      <h2>App reach</h2>
      <div class="sub">How far apps (napplets granted Mesh) may reach over the mesh. Two numbers
        because a flooded read costs every hop an answer as well as a forward, so it defaults lower.
        Zero keeps an app's traffic on this device.</div>
      ${hopsRow("Sending", "How far apps may send over the mesh", "pub", reach.publishTtl, reach.publishMax)}
      ${hopsRow("Fetching", "How far apps may look for what they missed", "sub", reach.subscribeTtl, reach.subscribeMax)}
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

// A hop-count stepper: − / value / +, clamped to 0..max. The value is
// worded, because "2" says nothing to someone who has never heard of a hop.
function hopsRow(title, subtitle, which, hops, max) {
  const wording =
    hops === 0 ? "This device only" : hops === 1 ? "Devices next to you (1 hop)" : `${hops} hops out`;
  return `<div class="row" style="margin-top:.6rem">
    <span class="grow"><div>${esc(title)}</div><div class="sub">${esc(subtitle)} — ${esc(wording)}</div></span>
    <span class="stepper">
      <button class="ghost" data-act="reach" data-which="${which}" data-delta="-1" ${hops <= 0 ? "disabled" : ""}>−</button>
      <span class="n">${hops}</span>
      <button class="ghost" data-act="reach" data-which="${which}" data-delta="1" ${hops >= max ? "disabled" : ""}>+</button>
    </span>
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

// The lanes a peer has a non-dead path on, in the fixed order Bluetooth,
// Aware, Network, each with whether fips currently sends on it. The Aware
// pool can hold several paths to one peer — that is one lane, lit if any of
// them is active. A core without paths shows the single transport as before.
const LANE_ORDER = ["ble", "aware", "udp"];
function peerLanes(p) {
  const live = (p.paths || []).filter((x) => x.state !== "dead" && x.lane);
  if (!live.length) return p.transport ? [[p.transport, true]] : [];
  const byLane = new Map();
  for (const x of live) byLane.set(x.lane, (byLane.get(x.lane) || false) || x.active);
  const rank = (l) => (LANE_ORDER.indexOf(l) < 0 ? LANE_ORDER.length : LANE_ORDER.indexOf(l));
  return [...byLane.entries()].sort((a, b) => rank(a[0]) - rank(b[0]));
}

// Line 2 of a peer row: the active lane first, standbys in brackets —
// `aware [ble]`. Nothing at all is an em-dash.
function pathSummary(p) {
  const lanes = peerLanes(p);
  if (!lanes.length) return "—";
  const active = lanes.filter((l) => l[1]).map((l) => l[0]).join(" ") || "—";
  const standby = lanes.filter((l) => !l[1]).map((l) => l[0]);
  return standby.length ? `${active} [${standby.join(" ")}]` : active;
}

// One path, fixed-width: lane, lifecycle state, then min RTT, sample count,
// ETX and score. `*` marks the path fips sends on, `b` a backup-role
// transport. Unmeasured values are dashes, never zeros. `min` is the window
// minimum, not srtt — a loaded active path shows a high ping elsewhere and a
// low min here, and that is why it has not switched.
function pathRow(x) {
  const mark = x.active ? "*" : x.role === "backup" ? "b" : " ";
  const min = x.minRttMs != null ? `${x.minRttMs}ms` : "—";
  const score = x.score != null ? x.score.toFixed(2) : "—";
  const line = `${mark} ${(x.lane || "").padEnd(5)} ${(x.state || "").padEnd(7)} min ${min} n=${x.rttSamples ?? 0} etx ${(x.etx ?? 0).toFixed(2)} score ${score}`;
  return `<div class="mono sub ${x.active ? "active" : ""}">${esc(line)}</div>`;
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
            <span class="sub">${esc(pathSummary(p))} · ${esc(p.state)} · ${agoText(p.lastSeenMs)}</span></span>
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
          ${(p.paths || []).length
            ? `<div class="paths"><div class="sub">paths</div>${p.paths.map(pathRow).join("")}</div>`
            : ""}
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
  invoke("open_nsite_window", { host, title }).catch((e) => askInfo("Could not open: " + e));
}

// The context menu lives outside #screen so re-renders can't wipe it.
const menu = document.getElementById("menu");

function shareApp(payload, title) {
  invoke("share_payload", payload)
    .then((p) =>
      ask(`Scan with Myco on another device to pair with you and open ${title} — or send the link.`, {
        html: `<div class="qr">${p.svg}</div>
               <input type="text" readonly value="${esc(p.uri)}" onfocus="this.select()" />`,
        yes: "Done",
        no: null,
      })
    )
    .catch((e) => askInfo(`Sharing failed: ${e}`));
}

function addToLauncher(link, title) {
  invoke("add_launcher_shortcut", { link, title })
    .then((path) => askInfo(`${title} is in your launcher now (${path}).`))
    .catch((e) => askInfo(`Failed to add the shortcut: ${e}`));
}

// The long-press sheet for a napplet — the same pull-up an nsite gets, because
// from the grid they are both just apps. What differs: no sync, and something
// an nsite never has — capabilities someone agreed to, which they should be
// able to see and take back.
function showNappletMenu(x, y, pointer, title) {
  menu.innerHTML = `
    <button data-act="open">Open</button>
    <button data-act="share">Share&#8230;</button>
    <button data-act="permissions">Manage permissions</button>
    <button data-act="launcher">Add to launcher</button>
    <button data-act="update">Check for updates</button>
    <button data-act="reload">Reload app</button>
    <button data-act="remove" class="danger">Remove app</button>`;
  menu.style.left = Math.min(x, window.innerWidth - 200) + "px";
  menu.style.top = Math.min(y, window.innerHeight - 280) + "px";
  menu.classList.remove("hidden");
  menu.onclick = (e) => {
    const act = e.target.closest("button")?.dataset.act;
    menu.classList.add("hidden");
    const item = installedNapplets().find((i) => i.pointer === pointer);
    if (act === "open" && item) openNapplet(item);
    if (act === "share") shareApp({ napplet: pointer }, title);
    if (act === "permissions") managePermissions(pointer, title);
    if (act === "launcher") addToLauncher(`myco://napplet/${pointer}`, title);
    if (act === "update") dispatch({ type: "check_nsite_updates" });
    // Fetches the app again and shows the same screen it was added with —
    // the way to revisit what it is allowed to do, or to force a re-fetch.
    if (act === "reload") dispatch({ type: "fetch_napplet", pointer });
    if (act === "remove") {
      askConfirm(`Remove ${title}? Its permissions are dropped; its files stay cached until the cache is cleared.`, "Remove").then((ok) => {
        if (ok) dispatch({ type: "forget_napplet", pointer });
      });
    }
  };
}

function showMenu(x, y, host, title) {
  menu.innerHTML = `
    <button data-act="open">Open</button>
    <button data-act="share">Share&#8230;</button>
    <button data-act="launcher">Add to launcher</button>
    <button data-act="update">Check for updates</button>
    <button data-act="remove" class="danger">Remove app</button>`;
  menu.style.left = Math.min(x, window.innerWidth - 200) + "px";
  menu.style.top = Math.min(y, window.innerHeight - 220) + "px";
  menu.classList.remove("hidden");
  menu.onclick = (e) => {
    const act = e.target.closest("button")?.dataset.act;
    menu.classList.add("hidden");
    if (act === "open") openApp(host, title);
    if (act === "share") shareApp({ host }, title);
    if (act === "launcher") addToLauncher(`myco://app/${host}`, title);
    if (act === "update") dispatch({ type: "check_nsite_updates" });
    if (act === "remove") {
      askConfirm(`Remove ${title}? Its files stay cached until the cache is cleared.`, "Remove").then((ok) => {
        if (ok) dispatch({ type: "forget_nsite", link: host });
      });
    }
  };
}

document.addEventListener("click", (e) => {
  if (!e.target.closest("#menu")) menu.classList.add("hidden");
});

// ------------------------------------------------------------------ wiring

document.getElementById("screen").addEventListener("click", (e) => {
  if (e.target.closest("#add-app")) {
    askPrompt("Paste an nsite link or host, or a napplet's naddr:").then((link) => {
      const text = (link || "").trim();
      if (!text) return;
      // An naddr is the honest signal for a napplet: Rust refuses one that
      // is not a napplet kind. It is fetched and reviewed, never installed
      // by a paste.
      if (/^naddr1/i.test(text)) dispatch({ type: "fetch_napplet", pointer: text });
      else dispatch({ type: "open_nsite", link: text });
    });
    return;
  }
  if (e.target.closest("#rename")) {
    askPrompt("Device name (what peers see when pairing):", deviceName).then((name) => {
      if (name && name.trim()) {
        invoke("rename_device", { name: name.trim() }).then((json) => {
          state = JSON.parse(json);
          refreshPairing();
        });
      }
    });
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
    if (kind === "remove") {
      askConfirm(`Remove ${name || npub} from your circle?`, "Remove").then((ok) => {
        if (ok) dispatch({ type: "remove_from_circle", npub });
      });
    }
    if (kind === "invite") {
      invoke("invite_peer", { npub, name: "" }).then((json) => {
        state = JSON.parse(json);
        render(true);
      });
    }
    if (kind === "speedtest") dispatch({ type: "speedtest_peer", npub });
    if (kind === "reach") {
      const reach = state.nappletMeshReach || {};
      const delta = Number(act.dataset.delta);
      let pub = reach.publishTtl || 0;
      let sub = reach.subscribeTtl || 0;
      if (act.dataset.which === "pub") pub = Math.max(0, Math.min(reach.publishMax ?? 255, pub + delta));
      else sub = Math.max(0, Math.min(reach.subscribeMax ?? 255, sub + delta));
      dispatch({ type: "set_napplet_mesh_reach", publishTtl: pub, subscribeTtl: sub });
    }
    if (kind === "send-file") {
      invoke("share_file_with_peer", { npub })
        .then((json) => {
          if (!json) return;
          state = JSON.parse(json);
          render(true);
        })
        .catch((err) => askInfo("Could not send: " + err));
    }
    const { id } = act.dataset;
    if (kind === "ft-accept") dispatch({ type: "accept_file_transfer", transferId: id });
    if (kind === "ft-decline") dispatch({ type: "decline_file_transfer", transferId: id });
    if (kind === "ft-cancel") dispatch({ type: "cancel_file_transfer", transferId: id });
    if (kind === "ft-forget") dispatch({ type: "forget_file_transfer", transferId: id });
    if (kind === "ls-start") {
      invoke("lanshare_start", { mode: lanshareMode })
        .then(refreshLanshare)
        .catch((err) => askInfo("Could not start: " + err));
    }
    if (kind === "ls-stop") invoke("lanshare_stop").then(refreshLanshare);
    if (kind === "ls-send") invoke("lanshare_send_files").then(refreshLanshare);
    if (kind === "wipe-cache") {
      askConfirm("Delete the cache? Files backing pinned apps are kept.", "Delete").then((ok) => {
        if (ok) dispatch({ type: "wipe_cache" });
      });
    }
    if (kind === "wipe-stores") {
      askConfirm(
        "Delete ALL data including apps? Identity and circle are kept. This cannot be undone.",
        "Delete everything"
      ).then((ok) => {
        if (ok) dispatch({ type: "wipe_stores" });
      });
    }
    return;
  }
  const napplet = e.target.closest(".tile[data-pointer]");
  if (napplet) {
    const item = installedNapplets().find((i) => i.pointer === napplet.dataset.pointer);
    if (item) openNapplet(item);
    return;
  }
  const suggestion = e.target.closest(".tile[data-fetch]");
  if (suggestion) {
    // The tap fetches and asks; the install action belongs to the review
    // sheet alone, so a suggestion can never grant a capability by itself.
    dispatch({ type: "fetch_napplet", pointer: suggestion.dataset.fetch });
    activateTab("apps");
    return;
  }
  const tile = e.target.closest(".tile[data-host]");
  if (tile) {
    const { host, title, holder } = tile.dataset;
    if (holder) {
      // Around-you: pull from the circle peer who holds it, then open.
      dispatch({ type: "open_nsite", link: host, holder });
      invoke("open_nsite_window", { host, title }).catch((err) => askInfo("Could not open: " + err));
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
  if (e.target.name === "ls-mode") lanshareMode = e.target.value;
});

document.getElementById("screen").addEventListener("contextmenu", (e) => {
  const napplet = e.target.closest(".tile[data-pointer]");
  if (napplet) {
    e.preventDefault();
    showNappletMenu(e.clientX, e.clientY, napplet.dataset.pointer, napplet.dataset.title);
    return;
  }
  const tile = e.target.closest(".tile[data-host]");
  if (!tile) return;
  e.preventDefault();
  showMenu(e.clientX, e.clientY, tile.dataset.host, tile.dataset.title);
});

// A permission switch on a napplet's sheet: live, and the only writer of a
// grant besides install review.
document.getElementById("modal").addEventListener("change", (e) => {
  const box = e.target.closest("input[data-grant]");
  const sheet = e.target.closest("[data-permissions]");
  if (!box || !sheet) return;
  box.closest(".cap")?.classList.toggle("off", !box.checked);
  dispatch({
    type: "set_napplet_grant",
    pointer: sheet.dataset.permissions,
    domain: box.dataset.grant,
    allowed: box.checked,
  });
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
  if (name === "circle") {
    refreshPairing();
    refreshLanshare();
  }
  // Entering Discover asks the circle what it holds, like the phone.
  if (name === "discover") dispatch({ type: "search_nsites" });
  render(true);
}

let sawFirstStateEvent = false;
listen("state", (event) => {
  if (!sawFirstStateEvent) {
    sawFirstStateEvent = true;
    uiLog("first state event received — event delivery works");
  }
  state = JSON.parse(event.payload);
  promptIncomingOffers();
  render();
});

// A finished receive, already moved to ~/Downloads/Myco by the backend.
listen("file-received", (event) => {
  const f = event.payload;
  if (f.error) {
    askInfo(`Received "${f.name}" but could not move it to Downloads (${f.error}). It is at ${f.path}.`);
  } else {
    askInfo(`${f.from || "A paired device"} sent you "${f.name}" — saved to ${f.path}.`);
  }
});

listen("pair-rotated", () => refreshPairing());

listen("goto", (event) => activateTab(event.payload));

listen("lanshare", () => refreshLanshare());

// The owner's half of every transfer: a guest wants to send (or take) a file.
listen("transfer-request", (event) => {
  const r = event.payload;
  uiLog(`transfer-request received: #${r.id} ${r.direction} ${r.name}`);
  const size = r.size ? ` (${fmtBytes(r.size)})` : "";
  const text =
    r.direction === "upload"
      ? `${r.from || "A guest"} wants to send you "${r.name}"${size}.`
      : `${r.from || "A guest"} wants to download "${r.name}"${size}.`;
  ask(text, { yes: "Accept", no: "Decline" }).then((v) => {
    invoke("lanshare_decide", { id: r.id, allow: v !== null });
  });
});

invoke("get_state").then((json) => {
  state = JSON.parse(json);
  render(true);
});

uiLog("shell ready — all listeners registered");

refreshPairing();
render();
