// History window: search, copy, paste again, delete.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let entries = [];
let query = "";
let exhausted = false;

const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });

function dayLabel(ts) {
  const d = new Date(ts);
  const today = new Date();
  const yesterday = new Date(Date.now() - 86400000);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return dayFmt.format(d);
}

function button(label, onClick) {
  const b = document.createElement("button");
  b.textContent = label;
  b.addEventListener("click", onClick);
  return b;
}

function render() {
  const list = $("list");
  list.replaceChildren();
  let lastDay = null;
  for (const e of entries) {
    const day = dayLabel(e.ts);
    if (day !== lastDay) {
      const h = document.createElement("li");
      h.className = "day";
      h.textContent = day;
      list.appendChild(h);
      lastDay = day;
    }
    const li = document.createElement("li");
    li.className = "entry";
    const text = document.createElement("div");
    text.className = "text";
    text.textContent = e.text;
    const meta = document.createElement("div");
    meta.className = "meta";
    const time = document.createElement("span");
    time.textContent = timeFmt.format(new Date(e.ts));
    const app = document.createElement("span");
    app.className = "app";
    app.textContent = e.app || "";
    const len = document.createElement("span");
    len.textContent = e.audio_ms ? `${(e.audio_ms / 1000).toFixed(1)} s` : "";
    meta.append(time, app, len);
    const actions = document.createElement("div");
    actions.className = "actions";
    const copy = button("Copy", async () => {
      await invoke("history_copy", { id: e.id });
      copy.textContent = "Copied";
      copy.classList.add("flash");
      setTimeout(() => { copy.textContent = "Copy"; copy.classList.remove("flash"); }, 1200);
    });
    copy.title = "Copy to the clipboard";
    const paste = button("Paste again", () => invoke("history_repaste", { id: e.id }));
    paste.title = "Paste into the app you were using before this window";
    const del = button("Delete", async () => {
      await invoke("history_delete", { id: e.id });
    });
    actions.append(copy, paste, del);
    li.append(text, meta, actions);
    list.appendChild(li);
  }
  const none = entries.length === 0;
  $("empty").hidden = !none;
  $("empty-text").textContent = query ? `No dictations match “${query}”.` : "Nothing here yet. Your dictations will show up here.";
  $("more").hidden = none || exhausted;
  $("summary").textContent = none ? "" : query ? `${entries.length}${exhausted ? "" : "+"} matching` : "";
}

async function load(reset = true) {
  const batch = await invoke("history_list", { query, offset: reset ? 0 : entries.length });
  entries = reset ? batch : entries.concat(batch);
  exhausted = batch.length < 100;
  render();
}

let searchTimer;
$("search").addEventListener("input", (e) => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { query = e.target.value.trim(); load(); }, 150);
});
$("more").addEventListener("click", () => load(false));

let confirming = false;
$("clear").addEventListener("click", async () => {
  if (!confirming) {
    confirming = true;
    $("clear").textContent = "Click again to delete everything";
    setTimeout(() => { confirming = false; $("clear").textContent = "Clear all"; }, 3000);
    return;
  }
  confirming = false;
  $("clear").textContent = "Clear all";
  await invoke("history_clear");
});

listen("history-changed", () => load());

(async () => {
  const s = await invoke("get_settings");
  $("retention").textContent = !s.history_enabled
    ? "History is paused."
    : s.history_days ? `Kept for ${s.history_days} days.` : "Kept until you delete it.";
  await load();
  $("search").focus();
})();
