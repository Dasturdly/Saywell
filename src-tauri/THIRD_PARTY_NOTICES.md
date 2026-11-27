# Third-party components

Saywell bundles or downloads the following. Each keeps its own license.

| Component | Used for | License |
|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (`whisper-server`, built with Vulkan) | speech recognition | MIT, © The ggml authors |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) (`llama-server`, release b12805, Vulkan) | text cleanup | MIT, © The ggml authors |
| LLVM OpenMP runtime (`libomp.dll`, shipped with llama.cpp) | CPU threading | Apache 2.0 with LLVM exception (see `LICENSE-LLVM-OpenMP`) |
| [Tauri](https://tauri.app), WebView2 | app shell and windows | MIT / Apache 2.0; WebView2 is part of Windows |
| SQLite (via rusqlite, bundled) | history and dictionary | public domain |

Models (downloaded on first run, not bundled):

| Model | License |
|---|---|
| Whisper large-v3-turbo, `ggml-large-v3-turbo-q5_0.bin` (OpenAI weights, ggml conversion by the whisper.cpp project) | MIT |
| Qwen3-4B-Instruct-2507, `Q4_K_M` GGUF (Alibaba Qwen weights, quantization by Unsloth) | Apache 2.0 |

Rust crate licenses are listed in `src-tauri/Cargo.lock` and can be printed with `cargo about` or `cargo license`.

## MIT License (whisper.cpp, llama.cpp)

Copyright (c) 2023-2026 The ggml authors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute,
sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT
OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
