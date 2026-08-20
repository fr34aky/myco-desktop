//! The guest page — a near-verbatim port of the phone's hotspot page
//! (`FileShareServer.kt`): framework-free HTML in the app's AMOLED theme,
//! one inline script doing raw-PUT uploads and AirDrop-style offer polling.

use super::ReceivedFile;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} kB", b as f64 / (1u64 << 10) as f64),
        b if b > 0 => format!("{b} B"),
        _ => String::new(),
    }
}

pub fn render(received: &[ReceivedFile]) -> String {
    let rows: String = received
        .iter()
        .map(|e| {
            format!(
                r#"<li><span class="fname">{}</span><span class="fsize">{}</span></li>"#,
                esc(&e.name),
                human(e.size)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let listing = if rows.is_empty() {
        r#"<p class="muted">No files shared this session yet.</p>"#.to_string()
    } else {
        format!("<ul class=\"files\">\n{rows}\n</ul>")
    };

    format!(
        r##"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="theme-color" content="#000000">
<title>Myco file share</title>
<style>
  :root {{ --bg:#000; --fg:#fff; --muted:#9ca3af; --accent:#34d399; --on-accent:#000;
          --outline:#3f3f46; --error:#ff6b6b; --surface:#101012; }}
  * {{ box-sizing: border-box; }}
  body {{ font-family: system-ui, sans-serif; background: var(--bg); color: var(--fg);
         margin: 0 auto; max-width: 30rem; padding: 1.4rem 1.2rem 3rem; }}
  header {{ display: flex; align-items: center; gap: .6rem; margin-bottom: .3rem; }}
  h1 {{ font-size: 1.35rem; font-weight: 800; margin: 0; }}
  .hint {{ color: var(--muted); font-size: .85rem; margin: .3rem 0 1.2rem; }}
  .label {{ color: var(--muted); font-weight: 700; font-size: .72rem; letter-spacing: .08em;
           text-transform: uppercase; margin: 1.7rem 0 .5rem; }}
  .muted {{ color: var(--muted); font-size: .9rem; }}
  ul.files {{ list-style: none; margin: 0; padding: 0; }}
  ul.files li {{ display: flex; justify-content: space-between; gap: 1rem; align-items: baseline;
                padding: .8rem .2rem; }}
  ul.files li + li {{ border-top: 1px solid var(--outline); }}
  .fname {{ word-break: break-all; font-weight: 600; }}
  .fsize {{ color: var(--muted); white-space: nowrap; font-size: .85rem; }}
  .pick-list {{ list-style: none; margin: .4rem 0 0; padding: 0; }}
  .pick-list li {{ display: flex; justify-content: space-between; gap: 1rem;
                  color: var(--muted); font-size: .85rem; padding: .25rem .2rem; }}
  .actions {{ display: flex; gap: .8rem; align-items: center; margin-top: .9rem; }}
  .btn {{ border-radius: 999px; padding: .65rem 1.4rem; font-size: .95rem; font-weight: 700;
         border: 1px solid transparent; font-family: inherit; cursor: pointer; }}
  .btn-primary {{ background: var(--accent); color: var(--on-accent); }}
  .btn-primary:disabled {{ background: #1f2937; color: #6b7280; }}
  .btn-outline {{ background: transparent; color: var(--accent); border-color: var(--outline); }}
  .btn-text-danger {{ background: none; border: none; color: var(--error); font-weight: 700;
                     font-family: inherit; font-size: .95rem; cursor: pointer; }}
  #fileInput {{ display: none; }}
  .overlay {{ position: fixed; inset: 0; background: rgba(0,0,0,.6); display: none;
             align-items: center; justify-content: center; }}
  .overlay.show {{ display: flex; }}
  .dialog {{ background: var(--surface); color: var(--fg); border: 1px solid var(--outline);
            border-radius: 1rem; padding: 1.2rem 1.2rem 1rem; max-width: 20rem; margin: 1rem;
            box-shadow: 0 8px 30px rgba(0,0,0,.5); }}
  .dialog h2 {{ margin: 0 0 .5rem; font-size: 1.05rem; }}
  .dialog p {{ margin: 0; }}
  .dialog .actions {{ justify-content: flex-end; margin-top: 1rem; }}
</style>
<header>
  <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#34d399" stroke-width="2" stroke-linecap="round" aria-hidden="true">
    <circle cx="12" cy="14" r="2" fill="#34d399" stroke="none"/>
    <path d="M8.5 10.5a5 5 0 0 1 7 0"/>
    <path d="M5.7 7.7a9 9 0 0 1 12.6 0"/>
  </svg>
  <h1>Share files with this computer</h1>
</header>
<p class="hint">Every transfer waits for an OK on the computer — give it a moment.</p>
<div class="label">Shared files</div>
{listing}
<div class="label">Send files to the computer</div>
<input type="file" id="fileInput" multiple>
<ul class="pick-list" id="pickList"></ul>
<div class="actions">
  <label for="fileInput" class="btn btn-outline">+ Choose files</label>
  <button class="btn btn-primary" id="sendBtn" disabled>Send</button>
</div>
<noscript><p class="muted">JavaScript is required — transfers wait for approval on the computer.</p></noscript>
<div class="overlay" id="offerOverlay">
  <div class="dialog">
    <h2>Incoming file</h2>
    <p id="offerText"></p>
    <div class="actions">
      <button class="btn-text-danger" id="offerDecline">Decline</button>
      <button class="btn btn-primary" id="offerAccept">Accept</button>
    </div>
  </div>
</div>
<script>
function humanSize(b) {{
  if (b >= 1048576) return (b / 1048576).toFixed(1) + ' MB';
  if (b >= 1024) return Math.round(b / 1024) + ' kB';
  return b > 0 ? b + ' B' : '';
}}
const fileInput = document.getElementById('fileInput');
const pickList = document.getElementById('pickList');
const sendBtn = document.getElementById('sendBtn');
fileInput.addEventListener('change', () => {{
  pickList.textContent = '';
  for (const f of fileInput.files) {{
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.textContent = f.name;
    const size = document.createElement('span');
    size.textContent = humanSize(f.size);
    li.append(name, size);
    pickList.append(li);
  }}
  sendBtn.disabled = fileInput.files.length === 0;
}});
sendBtn.addEventListener('click', async () => {{
  const files = fileInput.files;
  if (!files.length) return;
  sendBtn.disabled = true;
  sendBtn.textContent = 'Waiting for the OK…';
  let sent = 0;
  for (const f of files) {{
    try {{
      const r = await fetch('/upload/' + encodeURIComponent(f.name), {{method: 'PUT', body: f}});
      if (r.ok) sent++;
    }} catch (_) {{}}
  }}
  location.href = '/';
}});
const overlay = document.getElementById('offerOverlay');
const offerText = document.getElementById('offerText');
let current = null;
const handled = new Set();
function settle(accepted) {{
  if (!current) return;
  handled.add(current.id);
  if (accepted) {{
    window.location.href = '/offer/' + current.id;
  }} else {{
    fetch('/offer/' + current.id + '/decline', {{method: 'POST'}});
  }}
  overlay.classList.remove('show');
  current = null;
}}
document.getElementById('offerAccept').addEventListener('click', () => settle(true));
document.getElementById('offerDecline').addEventListener('click', () => settle(false));
async function pollOffers() {{
  try {{
    const r = await fetch('/offers');
    const data = await r.json();
    const next = data.offers.find(o => !handled.has(o.id));
    if (next && !current) {{
      current = next;
      const size = humanSize(next.size);
      offerText.textContent = 'The computer wants to send you "' + next.name + '"' +
        (size ? ' (' + size + ')' : '') + '.';
      overlay.classList.add('show');
    }}
  }} catch (e) {{}}
  setTimeout(pollOffers, 2000);
}}
pollOffers();
</script>
</html>"##
    )
}
