// First-run setup: download (or locate) the models, then a practice box.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const DESCRIPTIONS = {
  speech: "Turns your voice into text. Required.",
  cleanup: "Removes “um”, repeats and false starts. Optional; can be turned on later.",
};
const gb = (n) => (n / 1e9 >= 1 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.round(n / 1e6)} MB`);

let status = null;
let downloading = false;
let wantCleanup = null; // the user's choice for the optional model
const active = new Set(); // ids being downloaded
const failed = new Set();

function render() {
  const box = $("models");
  box.replaceChildren();
  for (const m of status.models) {
    const row = document.createElement("div");
    row.className = "model" + (m.present ? " present" : "") + (active.has(m.id) ? " downloading" : "") + (failed.has(m.id) ? " failed" : "");
    row.id = `m-${m.id}`;
    const optional = m.id === "cleanup";
    row.innerHTML = `
      <label class="check"><input type="checkbox"></label>
      <div class="name"></div>
      <div class="size"></div>
      <div class="sub"></div>
      <div class="meterbar"></div>`;
    const cb = row.querySelector("input");
    cb.checked = m.present || !optional || wantCleanup;
    cb.disabled = m.present || !optional || downloading;
    cb.setAttribute("aria-label", m.label);
    cb.addEventListener("change", () => {
      if (optional) wantCleanup = cb.checked;
      update();
    });
    row.querySelector(".name").textContent = m.label;
    row.querySelector(".size").textContent = m.present ? "✓ Ready" : gb(m.size);
    row.querySelector(".sub").textContent = DESCRIPTIONS[m.id] ?? "";
    box.appendChild(row);
  }
  $("models_dir").textContent = status.models_dir;
  update();
}

function wanted() {
  return status.models.filter((m) => !m.present && (m.id !== "cleanup" || wantCleanup));
}

function update() {
  const need = wanted();
  const speechReady = status.models.find((m) => m.id === "speech").present;
  const total = need.reduce((a, m) => a + m.size, 0);
  $("total").textContent = need.length ? `${gb(total)} to download` : "Everything you picked is here.";
  $("download").hidden = need.length === 0;
  $("download").disabled = downloading;
  $("download").textContent = downloading ? "Downloading…" : "Download";
  $("continue").hidden = !(need.length === 0 && speechReady);
}

$("download").addEventListener("click", async () => {
  $("dl-error").textContent = "";
  const ids = wanted().map((m) => m.id);
  try {
    await invoke("start_download", { ids });
    downloading = true;
    failed.clear();
    ids.forEach((id) => active.add(id));
    render();
  } catch (e) {
    $("dl-error").textContent = String(e);
  }
});

listen("download", async ({ payload: d }) => {
  const row = document.querySelector(`#m-${d.id}`);
  if (d.state === "progress" && row) {
    row.classList.add("downloading");
    row.style.setProperty("--p", `${((100 * d.done) / d.total).toFixed(1)}%`);
    row.querySelector(".size").textContent = `${gb(d.done)} of ${gb(d.total)}`;
    return;
  }
  active.delete(d.id);
  if (d.state === "error") {
    downloading = false;
    active.clear();
    failed.add(d.id);
    $("dl-error").textContent = `Download stopped: ${d.error}. Click Download to resume.`;
  }
  status = await invoke("setup_status");
  if (active.size === 0) downloading = false;
  render();
});

$("open-dir").addEventListener("click", () => invoke("open_models_dir"));
$("recheck").addEventListener("click", async () => {
  status = await invoke("setup_status");
  render();
});

$("continue").addEventListener("click", async () => {
  const cleanup = !!wantCleanup && status.models.find((m) => m.id === "cleanup").present;
  try {
    await invoke("finish_setup", { cleanup });
  } catch (e) {
    $("dl-error").textContent = String(e);
    return;
  }
  $("step-models").hidden = true;
  $("step-try").hidden = false;
  $("step-label").textContent = "Step 2 of 2 · Try it";
  $("key").textContent = status.hotkey_label;
  showEngine(await invoke("engine_status"));
  $("try").focus();
});

function showEngine(text) {
  const ready = text.startsWith("ready");
  $("engine").textContent = ready ? "Ready. Go ahead." : text ? text.charAt(0).toUpperCase() + text.slice(1) : "Starting…";
  $("engine-dot").classList.toggle("ready", ready);
}
listen("status", ({ payload }) => showEngine(payload));

$("done").addEventListener("click", () => invoke("close_setup"));

// Dictation into this window arrives as an event (pasting into our own window isn't reliable).
listen("insert-text", ({ payload }) => {
  const box = $("try");
  if ($("step-try").hidden) return;
  const start = box.selectionStart ?? box.value.length;
  const before = box.value.slice(0, start);
  const sep = before && !/\s$/.test(before) ? " " : "";
  box.setRangeText(sep + payload, start, box.selectionEnd ?? start, "end");
  box.focus();
});

(async () => {
  status = await invoke("setup_status");
  wantCleanup = status.cleanup_enabled;
  render();
  if (status.engines_started) $("continue").click();
})();
