#!/usr/bin/env bash
# M0: cleanup-LLM latency on sample messy transcripts via llama-server (Vulkan, all layers on GPU).
# Usage: scripts/bench-llm.sh <llama-server.exe> <model.gguf>
set -euo pipefail
SRV=$1; MODEL=$2; ROOT=$(cd "$(dirname "$0")/.." && pwd); OUT=$(mktemp -d); PORT=18182
"$SRV" -m "$MODEL" -ngl 99 -c 4096 --host 127.0.0.1 --port $PORT >"$OUT/server.log" 2>&1 &
PID=$!; trap 'kill $PID 2>/dev/null || true' EXIT
for _ in $(seq 120); do curl -sf "http://127.0.0.1:$PORT/health" >/dev/null && break; sleep 1; done
grep -iE "ggml_vulkan|offloaded|Vulkan0" "$OUT/server.log" | head -4 || true
python "$ROOT/scripts/llm_cleanup_probe.py" "http://127.0.0.1:$PORT" "$ROOT/tests/fixtures/cleanup/cases.tsv"
