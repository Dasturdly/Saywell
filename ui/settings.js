// Settings window. Every change is applied and saved immediately.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let current = null;

function render(s) {
  current = s;
  $("keycap").textContent = s.hotkey_label;
  $("key-desc").textContent = `Hold ${s.hotkey_label}, speak, let go. The text is typed where your cursor is.`;
  for (const k of ["double_tap_hands_free", "cleanup_enabled", "preview_caption", "start_with_windows", "history_enabled"]) $(k).checked = s[k];
  $("history_days").value = String(s.history_days);
  const mic = $("mic");
  mic.replaceChildren();
  const def = new Option("Windows default", "");
  mic.add(def);
  for (const name of s.mics) mic.add(new Option(name, name));
  if (s.mic_device && !s.mics.includes(s.mic_device)) mic.add(new Option(`${s.mic_device} (not connected)`, s.mic_device));
  mic.value = s.mic_device ?? "";
  $("models_dir").textContent = s.models_dir;
  $("version").textContent = `Version ${s.version}`;
}

let savedTimer;
async function save(patch) {
  try {
    render(await invoke("update_settings", { patch }));
    $("key-error").textContent = "";
    const el = $("saved");
    el.textContent = "Saved";
    el.classList.add("show");
    clearTimeout(savedTimer);
    savedTimer = setTimeout(() => el.classList.remove("show"), 1400);
  } catch (e) {
    $("key-error").textContent = String(e);
    render(await invoke("get_settings"));
  }
}

for (const k of ["double_tap_hands_free", "cleanup_enabled", "preview_caption", "start_with_windows", "history_enabled"]) {
  $(k).addEventListener("change", (e) => save({ [k]: e.target.checked }));
}
$("mic").addEventListener("change", (e) => save({ mic_device: e.target.value }));
$("history_days").addEventListener("change", (e) => save({ history_days: Number(e.target.value) }));
$("open-history").addEventListener("click", () => invoke("open_history"));

// ---- dictionary ----
function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

function removeButton(label, onClick) {
  const x = el("button", "", "\u2715");
  x.setAttribute("aria-label", label);
  x.addEventListener("click", onClick);
  return x;
}

function renderDict(d) {
  $("terms").replaceChildren(...d.terms.map((t) => {
    const li = el("li", "", t.term);
    li.appendChild(removeButton(`Remove ${t.term}`, async () => renderDict(await invoke("dict_remove_term", { id: t.id }))));
    return li;
  }));
  $("rules").replaceChildren(...d.rules.map((r) => {
    const li = el("li");
    li.append(
      el("span", "find", `\u201c${r.find}\u201d`),
      el("span", "to", "\u2192"),
      el("span", "rep", r.replace || "(nothing)"),
      removeButton(`Remove replacement for ${r.find}`, async () => renderDict(await invoke("dict_remove_rule", { id: r.id }))),
    );
    return li;
  }));
}

async function dictCall(cmd, args) {
  try {
    renderDict(await invoke(cmd, args));
    $("dict-error").textContent = "";
    return true;
  } catch (e) {
    $("dict-error").textContent = String(e);
    return false;
  }
}

$("term-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const v = $("term").value.trim();
  if (v && (await dictCall("dict_add_term", { term: v }))) $("term").value = "";
});
$("rule-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (await dictCall("dict_set_rule", { find: $("find").value, replace: $("replace").value })) {
    $("find").value = "";
    $("replace").value = "";
    $("find").focus();
  }
});

// ---- hotkey capture (saywellKeyFor comes from keys.js) ----

let capturing = false;
async function stopCapture() {
  capturing = false;
  window.saywellCapturingKey = false;
  $("keycap").classList.remove("listening");
  window.removeEventListener("keydown", onKey, true);
  await invoke("capture_hotkey", { active: false });
  if (current) $("keycap").textContent = current.hotkey_label;
}

async function onKey(e) {
  e.preventDefault();
  e.stopPropagation();
  if (e.repeat) return;
  if (e.key === "Escape") return stopCapture();
  const k = saywellKeyFor(e);
  if (!k) {
    $("key-error").textContent = "Pick a key you don't type with: Ctrl, Alt or Shift (left or right side), an F key, Pause, Insert, Scroll Lock or Menu.";
    return;
  }
  await stopCapture();
  save({ hotkey_vk: k[0], hotkey_label: k[1] });
}

$("keycap").addEventListener("click", async () => {
  if (capturing) return stopCapture();
  capturing = true;
  window.saywellCapturingKey = true;
  $("key-error").textContent = "";
  $("keycap").textContent = "Press a key…";
  $("keycap").classList.add("listening");
  await invoke("capture_hotkey", { active: true });
  window.addEventListener("keydown", onKey, true);
});
window.addEventListener("blur", () => capturing && stopCapture());

// ---- status ----
function showStatus(text) {
  const el = $("status");
  el.textContent = text ? text.charAt(0).toUpperCase() + text.slice(1) : "";
  el.className = "status" + (text.startsWith("ready") ? " ready" : text.includes("loading") || text.includes("starting") ? " busy" : "");
}
listen("status", ({ payload }) => showStatus(payload));

(async () => {
  render(await invoke("get_settings"));
  showStatus(await invoke("engine_status"));
  renderDict(await invoke("dict_get"));
})();
