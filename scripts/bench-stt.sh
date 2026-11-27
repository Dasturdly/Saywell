#!/usr/bin/env bash
# M0: warm-model STT latency + WER. Starts whisper-server once, then POSTs every fixture clip.
# Usage: scripts/bench-stt.sh <whisper-server.exe> <model.bin> [extra server args]
set -euo pipefail
SRV=$1; MODEL=$2; shift 2
ROOT=$(cd "$(dirname "$0")/.." && pwd); A=$ROOT/tests/fixtures/audio; OUT=$(mktemp -d); PORT=18181
"$SRV" -m "$MODEL" -l en --host 127.0.0.1 --port $PORT "$@" >"$OUT/server.log" 2>&1 &
PID=$!; trap 'kill $PID 2>/dev/null || true' EXIT
for _ in $(seq 60); do curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 1; done
grep -iE "vulkan|ggml_vk|using .* backend|device" "$OUT/server.log" | head -5 || true
# warm-up request
curl -s -o /dev/null -F file=@"$A/tts01.wav" -F response_format=text "http://127.0.0.1:$PORT/inference"
printf "%-6s %8s %8s %7s\n" clip audio_s latency wer
for wav in "$A"/tts*.wav; do
  id=$(basename "$wav" .wav)
  dur=$(python -c "import wave,sys;w=wave.open(sys.argv[1]);print(round(w.getnframes()/w.getframerate(),2))" "$wav")
  t=$(curl -s -o "$OUT/$id.txt" -w '%{time_total}' -F file=@"$wav" -F response_format=text -F temperature=0 "http://127.0.0.1:$PORT/inference")
  w=$(python "$ROOT/scripts/wer.py" "$A/$id.txt" "$OUT/$id.txt")
  printf "%-6s %8s %8.3f %7s\n" "$id" "$dur" "$t" "$w"
done | tee "$OUT/results.txt"
sort -k3 -n "$OUT/results.txt" | awk '{l[NR]=$3; w+=$4; a+=$2} END{printf "clips=%d  mean_audio=%.1fs  p50=%.3fs  p95=%.3fs  mean_WER=%.2f%%\n", NR, a/NR, l[int((NR+1)/2)], l[int(NR*0.95+0.5)], 100*w/NR}'
echo "outputs: $OUT"
