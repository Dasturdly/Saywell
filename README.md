# Saywell

Private, offline voice dictation for Windows 11. Hold a key, speak, let go: the words are typed
into whatever app you're using. Speech recognition (Whisper) and text cleanup (a small local
language model) run on your PC's graphics card. Nothing you say or type leaves the computer.

## Install
1. Run `Saywell_0.1.0_x64-setup.exe` (no admin rights needed).
2. First-run setup downloads the two model files (about 3.1 GB), or point it at a folder that
   already has them ("I already have the files").
3. Hold **Right Ctrl**, speak, release.

## Use
- **Hold to talk**: hold the key while you speak. **Double-tap** for hands-free; tap again to finish. **Esc** cancels.
- The floating bar shows a live waveform, the time, and your latest words.
- Tray icon → **Settings**: dictation key, microphone, AI cleanup on/off, caption, start with Windows,
  dictionary (words and replacements), per-app "type instead of paste", history.
- Tray icon → **History** or **Paste again**: find, copy or re-insert past dictations.
- Admin ("Run as administrator") windows can't be typed into by normal apps; Saywell copies the
  text and tells you to press Ctrl+V.

Data lives in `%APPDATA%\Saywell` (settings, models, `saywell.db` history, logs). Logs never
contain what you said.

## Build from source
Needs Rust (MSVC), the helper binaries in `src-tauri/binaries/` (whisper.cpp `whisper-server`
built with `-DGGML_VULKAN=ON`, llama.cpp `llama-server` Vulkan release) and the models in
`models/` (git-ignored). See `PLAN.md` and `docs/benchmarks.md`.

```
cd src-tauri
cargo build                     # debug build
cargo test --lib                # unit tests
cargo test --test pipeline --test cleanup -- --ignored   # needs models and helpers
cargo tauri build --bundles nsis                         # installer
```
End-to-end tests: `scripts/e2e-paste.ps1` (uses a throwaway profile and F24 as the hotkey, so it
never reacts to your own keys) and `scripts/offline-audit.ps1`.

Third-party licenses: `src-tauri/THIRD_PARTY_NOTICES.md`.
