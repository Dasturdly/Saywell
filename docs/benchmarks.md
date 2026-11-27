# M0 benchmarks (2026-11-27)

Machine: Ryzen 5 5600X, 32 GB RAM, AMD Radeon RX 9070 XT 16 GB (driver 26.10.44, Vulkan 1.4.349, KHR_coopmat).
Everything runs warm (model already loaded), on the GPU through Vulkan, over loopback HTTP.

## Speech-to-text: whisper.cpp `79b1191` (built from source, `-DGGML_VULKAN=ON`)
Model `ggml-large-v3-turbo-q5_0.bin` (573 MB in VRAM), `-l en`, temperature 0.
Run with `scripts/bench-stt.sh`: 20 SAPI TTS clips (David and Zira voices), each about 4 s.

| metric | result |
|---|---|
| latency p50 / p95 | **0.173 s / 0.186 s** per clip (≈ 24× real time) |
| raw WER | 4.16% |
| real WER | **0%**. All mismatches are numbers written as digits (`forty percent` → `40%`, `six fifteen` → `6:15`) |

## Cleanup LLM: llama.cpp `b12805` win-vulkan-x64 (prebuilt)
Model `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` (2.5 GB), `-ngl 99 -c 4096`, temperature 0.
Run with `scripts/bench-llm.sh` on 6 cases in `tests/fixtures/cleanup/cases.tsv`.

| metric | result |
|---|---|
| latency p50 / max | **0.187 s / 0.599 s** (the max is the first real request) |
| correctness | 6/6 acceptable. Self-correction ("3, no wait, 4") handled, a question was cleaned rather than answered, lists formatted |
| guardrail | 6/6 passed |

Note: llama.cpp warns that flash attention for K/V falls back to CPU. Try `-fa off` versus the default in M2.

## Estimated end to end (4 s utterance)
STT 0.17 s + cleanup ~0.2 s + paste ~0.05 s ≈ **0.4–0.5 s** from key release, well under the 1.8 s target.
Not yet measured: clips of 10 s or longer, your real voice and mic, and VRAM under load (estimated ≈ 4 GB total).

## Decision
Go. **Both engines run as loopback sidecars** (`whisper-server.exe` and `llama-server.exe`) instead of linking `whisper-rs` in-process:
it's already proven at 0.17 s, a GPU or driver crash can't take down the UI, and the Rust build stays plain (no Vulkan SDK needed to `cargo build`).
The Vulkan SDK is only needed to rebuild whisper.cpp.
