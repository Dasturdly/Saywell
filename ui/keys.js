// Shared by Saywell's own windows.
//
// While one of these windows has focus, the embedded browser engine can take keyboard input
// before Saywell's global key hook sees it. So the page forwards the keys that can be the
// dictation key (side-specific Ctrl/Alt/Shift, F-keys, Pause, Insert, Scroll Lock, Menu) and Esc.
// Ordinary typing is never sent anywhere.

const SIDE = { 16: ["Shift", 0xa0, 0xa1], 17: ["Ctrl", 0xa2, 0xa3], 18: ["Alt", 0xa4, 0xa5] };
const NAMED = { 19: "Pause", 45: "Insert", 93: "Menu", 145: "Scroll Lock" };

/** Map a key event to [virtual-key code, label], or null if it can't be a dictation key. */
function saywellKeyFor(e) {
  const side = SIDE[e.keyCode];
  if (side) {
    if (e.location === 2) return [side[2], `Right ${side[0]}`];
    if (e.location === 1) return [side[1], `Left ${side[0]}`];
    return null;
  }
  if (e.keyCode >= 112 && e.keyCode <= 135) return [e.keyCode, `F${e.keyCode - 111}`];
  if (NAMED[e.keyCode]) return [e.keyCode, NAMED[e.keyCode]];
  return null;
}

(() => {
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) return;
  const forward = (down) => (e) => {
    if (window.saywellCapturingKey || e.repeat) return;
    const vk = e.key === "Escape" ? 0x1b : saywellKeyFor(e)?.[0];
    if (vk !== undefined) invoke("page_key", { vk, down }).catch(() => {});
  };
  window.addEventListener("keydown", forward(true));
  window.addEventListener("keyup", forward(false));
})();
