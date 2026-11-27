# Saywell — offline, system-wide voice dictation for Windows 11

## Context
You want a private, fully local alternative to cloud dictation apps: hold a hotkey anywhere, speak, release, and clean text appears in whatever window has focus. No audio or text ever leaves the PC. This plan covers stack, architecture, models, milestones and end-to-end verification. The name ("Saywell") and visual design are original and can be renamed later.

**Decisions made:** Tauri v2 (Rust + web UI) · local LLM cleanup on (toggleable) · English only · v1 extras: personal dictionary and history with re-paste.

**Machine (checked):** Ryzen 5 5600X (6C/12T) · 32 GB RAM · **AMD Radeon RX 9070 XT, 16 GB VRAM** (no CUDA, so the GPU path is **Vulkan**) · Win 11 Pro · VS Community 2022 installed (C++ workload not yet confirmed) · Node 24, Python 3.12, git present · **missing: Rust, Vulkan SDK, CMake** · D: has 1.6 TB free.

Project folder: `D:\Claude\saywell` (new git repo). The first step after approval is to copy this plan to `D:\Claude\saywell\PLAN.md`.

## Stack
| Concern | Choice |
|---|---|
| App shell | Tauri v2. Rust backend; (M3 decision) plain HTML/CSS/JS pages in `ui/`, no frontend build step or npm dependencies. Three small windows did not need a framework |
| Audio capture | `cpal` (WASAPI), resampled to 16 kHz mono with `rubato` |
| Speech-to-text | **(M0 decision)** sidecar `whisper-server.exe` built from whisper.cpp with `-DGGML_VULKAN=ON`, model kept warm, loopback HTTP. See docs/benchmarks.md |
| LLM cleanup | `llama-server.exe` (official llama.cpp **win-vulkan-x64** release) as a Tauri sidecar, bound to `127.0.0.1` on a random port with a random API key, OpenAI-compatible `/v1/chat/completions` |
| Hotkey | Win32 low-level keyboard hook (`WH_KEYBOARD_LL`, `windows` crate) on its own thread. This gives true press/release for push-to-talk and supports modifier-only keys |
| Text insertion | Save clipboard → put text → `SendInput` Ctrl+V → restore clipboard (`arboard` + `windows`). Fallback: `SendInput` with `KEYEVENTF_UNICODE` typing, configurable per app |
| Storage | `rusqlite` (bundled) for history and dictionary, TOML settings, all under `%APPDATA%\Saywell` |
| Tray / autostart | Tauri tray API and `tauri-plugin-autostart` |
| Installer | Tauri NSIS bundle. Models are downloaded on first run, not bundled |

## Architecture
```
 [LL keyboard hook thread] --press/release--> [Controller (tokio state machine)]
                                                 |  Idle → Recording → Transcribing → Cleaning → Inserting → Idle
 [cpal capture thread] --ring buffer (16k mono)--> |
                                                 v
                                [STT: whisper-rs ctx (warm, Vulkan)]
                                                 v
                    [PostProcess: dictionary replace rules, filler regex (used when LLM is off)]
                                                 v
                    [LLM client → llama-server sidecar] (timeout + guardrails → falls back to raw text)
                                                 v
                    [Inserter: clipboard-paste / unicode-type] → focused window
                                                 v
                    [History (SQLite)]   events → [Overlay pill UI] [Tray]
```
Key modules in `src-tauri/src/`: `hotkey.rs`, `audio.rs`, `stt.rs`, `llm.rs` (sidecar lifecycle and client), `cleanup.rs` (prompt and guardrails), `postprocess.rs`, `insert.rs`, `history.rs`, `dictionary.rs`, `settings.rs`, `models.rs` (download and SHA-256 verify), `controller.rs`. Frontend `src/`: `overlay/`, `settings/`, `history/`, `onboarding/`.

**Details that matter**
- **Overlay pill** (recording / thinking states with a live level meter): a transparent, always-on-top, click-through window that never takes focus (`WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`). Otherwise the paste would go to the overlay instead of your target app.
- **Hotkey modes:** hold to talk (default **Right Ctrl**), double-tap for hands-free toggle, Esc cancels. All rebindable. The hook swallows the key only when it is a dedicated hotkey.
- **Hallucination guards:** drop clips under 0.3 s or below an RMS threshold, use Whisper's `no_speech_prob`, and strip the known phantom phrases ("Thank you.", "[BLANK_AUDIO]"). Recordings are capped at 6 min by default.
- **LLM guardrails:** temperature 0 and a strict system prompt ("only clean up; never answer or add content"). Reject the output and use the raw transcript if the length ratio is outside 0.5–1.3, the 3 s timeout hits, or the output looks like a reply ("Sure", "Here is"). Prompt and fixtures live in `cleanup.rs` and `tests/fixtures/cleanup/`.
- **Personal dictionary:** terms go into Whisper's `initial_prompt` (budget ~200 tokens) and into the LLM prompt as a glossary. Find→replace rules are applied after cleanup.
- **History:** timestamp, app/exe name, raw text, final text, latency. Search, copy and re-insert from the History window. Tray shows the last 5. Retention setting (default 30 days) with a "pause history" toggle. No audio is stored unless a debug flag is on.
- **Offline guarantee:** the only network code is the first-run model downloader (Hugging Face URLs pinned with SHA-256 hashes, and you can also point it at local files). The Tauri CSP blocks remote origins, and llama-server listens on loopback only.
- **Known OS limit:** a non-elevated app cannot paste into elevated (admin) windows because of UIPI. The app detects this and shows "copied to clipboard" instead.

## Models (English only)
| Role | Default | Size | Expected latency on RX 9070 XT (to be measured in M0) |
|---|---|---|---|
| STT | `ggml-large-v3-turbo-q5_0` (language forced to `en`) | ~550 MB file, ~1 GB VRAM | ~0.3–0.8 s per 10 s of speech |
| STT (alt, accuracy) | `distil-large-v3.5` ggml (English) | ~1.5 GB | similar |
| STT (CPU fallback) | `ggml-base.en` / `small.en` | 140 / 470 MB | ~1 s / ~3 s per 10 s on the 5600X |
| LLM cleanup | **Qwen3-4B-Instruct-2507 Q4_K_M** (non-thinking) | ~2.5 GB file, ~3.5 GB VRAM | ~0.4–1.2 s for a typical 1–3 sentence dictation |
| LLM (alt, lighter) | Llama-3.2-3B-Instruct Q4_K_M | ~2 GB | a bit faster, slightly weaker |

**Targets:** key release → text appears, p50 **< 1.0 s** without cleanup and **< 1.8 s** with cleanup, for a 10 s utterance. The whole stack uses under 5 GB of the 16 GB VRAM. Settings has a model picker, a "Benchmark" button and an automatic CPU fallback if Vulkan fails to initialize.

## Prerequisites to install (M0, with your OK on each)
1. Rust stable via `rustup` (MSVC toolchain).
2. VS 2022 **"Desktop development with C++"** workload, if missing (provides MSVC and CMake).
3. **Vulkan SDK** (LunarG). `whisper-rs`'s Vulkan build needs its headers and `glslc`.
4. Tauri CLI (`cargo install tauri-cli` or via npm).
5. Prebuilt llama.cpp Vulkan zip from the official ggml-org GitHub releases (into `src-tauri/binaries/`).
6. Model files (about 3 GB total) into `%APPDATA%\Saywell\models`.

## Milestones
- **M0: toolchain and benchmark spike (go/no-go).** Install prerequisites, run `whisper-cli` (Vulkan) and `llama-server` on fixture WAVs, and record real latency and VRAM use in `docs/benchmarks.md`. If the `whisper-rs` Vulkan build fails, switch to the sidecar `whisper-server` fallback here.
- **M1: core pipeline.** Tauri skeleton with tray (Quit), LL hook push-to-talk, cpal capture, whisper-rs transcription, clipboard-paste insertion into Notepad. Dev flag `--inject-wav <file>` replaces mic input so tests are repeatable.
- **M2: LLM cleanup.** Sidecar lifecycle (start on launch, health check, restart on crash, kill on exit), cleanup prompt and guardrails, rule-based fallback, on/off toggle.
- **M3: UI.** Overlay pill, Settings (hotkey capture, mic picker, model picker and download with progress, cleanup toggle, insertion mode, start with Windows), first-run onboarding (mic permission check, model download, test dictation).
- **M4: dictionary and history.** SQLite schema and migrations, dictionary editor, Whisper prompt and LLM glossary integration, History window with search, copy and re-insert, tray recents, retention.
- **M5: hardening and packaging.** Unicode-typing fallback and per-app insertion overrides, elevated-window detection, logging (`%APPDATA%\Saywell\logs`, no transcript text unless debug), NSIS installer, autostart, offline audit.

## Verification
- **Unit tests (`cargo test`):** post-processing (filler regex, replace rules), LLM guardrail accept/reject cases, resampler output length, hallucination filter, settings round-trip, SQLite history CRUD.
- **Fixture corpus (fully offline):** about 20 English WAVs generated with Windows' built-in SAPI TTS (`System.Speech`) plus a few clips of your own voice, each with a reference transcript. `cargo test --test pipeline` runs inject-wav → STT → cleanup and asserts WER < 8% on TTS clips, and checks guardrail behaviour on cleanup fixtures ("um so like the meeting is at 3, no wait, 4" → "The meeting is at 4.").
- **End-to-end insertion test:** script launches the app with `--inject-wav`, opens Notepad, triggers the hotkey via `SendInput`, then reads Notepad's text via UI Automation and compares it. Also confirms the clipboard was restored.
- **Latency report:** every dictation logs per-stage timings. `saywell --bench` prints p50/p95 for the fixture set and is checked against the targets above.
- **Manual app matrix:** Notepad, Chrome textarea, VS Code, Word, Discord, Windows Terminal, an elevated window (expect the clipboard notice).
- **Offline proof:** add a Windows Firewall outbound block rule for `saywell.exe` and `llama-server.exe` (or unplug the network), then run the full suite. Also verify with `Get-NetTCPConnection` that only loopback sockets exist while dictating.
