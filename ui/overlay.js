// Compact bar: state, a scrolling waveform from the mic level, and an optional caption.
const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const msg = document.getElementById("msg");
const caption = document.getElementById("caption");
const wave = document.getElementById("wave");
const timeEl = document.getElementById("time");
let startedAt = 0;
const CAPTION_CHARS = 110; // about two lines

const BARS = 26;
for (let i = 0; i < BARS; i++) wave.appendChild(document.createElement("i"));
const bars = [...wave.children];
const history = new Array(BARS).fill(0);
let smooth = 0;

const live = () => pill.dataset.state === "listening" || pill.dataset.state === "handsfree";

function draw() {
  bars.forEach((b, i) => (b.style.height = `${Math.round(3 + 19 * history[i])}px`));
}

// Show the end of the text (where you'll continue from), starting at a word boundary.
function setCaption(t) {
  caption.textContent = "";
  if (t) {
    let tail = t;
    if (t.length > CAPTION_CHARS) {
      tail = t.slice(-CAPTION_CHARS);
      tail = "…" + tail.slice(tail.indexOf(" ") + 1);
    }
    caption.textContent = tail;
  }
  caption.classList.toggle("show", !!t);
}

function tickTime() {
  const s = Math.floor((Date.now() - startedAt) / 1000);
  timeEl.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

listen("overlay", ({ payload }) => {
  const { state, text } = payload;
  const wasLive = live();
  pill.dataset.state = state;
  msg.textContent = state === "error" || state === "notice" ? text : "";
  // Keep the caption while the final text is written, so you can see where you stopped.
  if (state === "hidden" || state === "error" || state === "notice") setCaption("");
  if (live() && !wasLive) {
    history.fill(0);
    smooth = 0;
    draw();
    setCaption("");
    startedAt = Date.now();
    tickTime();
  }
});

listen("overlay-preview", ({ payload }) => {
  if (live()) setCaption(payload);
});

// RMS (0..1) on a log scale: quiet speech ~0.01, loud ~0.2. New values enter on the right.
listen("overlay-level", ({ payload }) => {
  if (!live()) return;
  const v = Math.max(0, Math.min(1, Math.log10(Math.max(payload, 1e-4) * 1000) / 2.3));
  smooth = v > smooth ? v * 0.7 + smooth * 0.3 : smooth * 0.6 + v * 0.4;
  tickTime();
  history.shift();
  history.push(Math.min(1, smooth * (0.8 + Math.random() * 0.4)));
  draw();
});
