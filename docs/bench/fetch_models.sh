#!/usr/bin/env bash
# Resumable model fetch for the S7 model-floor measurement.
#
# HuggingFace supports range requests, so each attempt continues where the
# last stopped; the loop exists because the link drops, not because the server
# refuses. On the machine this was written for HuggingFace served ~51 KB/s, so
# the 0.5B lands in roughly 3 hours and the 3B in roughly 11.
#
# `wc -c` rather than `stat -c%s`: the latter is GNU-only and this has to run
# on a plain macOS shell too.
set -u
# Models land in .bench/models/, which is gitignored: weights are large and
# are reproducible from the URLs below.
cd "$(dirname "$0")/../../.bench/models" 2>/dev/null || { mkdir -p "$(dirname "$0")/../../.bench/models"; cd "$(dirname "$0")/../../.bench/models"; }
fetch() {
  local url="$1" out="$2"
  local want
  want=$(curl -sSI -L --ssl-no-revoke --max-time 60 "$url" 2>/dev/null | grep -i '^content-length' | tail -1 | tr -d '\r' | awk '{print $2}')
  [ -z "$want" ] && { echo "$(date +%T) $out: cannot determine size, skipping"; return 1; }
  echo "$(date +%T) $out: target ${want} bytes"
  for attempt in $(seq 1 400); do
    local have=0
    [ -f "$out" ] && have=$(wc -c < "$out" 2>/dev/null | tr -d " " || echo 0)
    if [ "$have" -ge "$want" ]; then
      echo "$(date +%T) $out: COMPLETE ($have bytes)"
      return 0
    fi
    curl -sS -L --ssl-no-revoke -C - --max-time 600 -o "$out" "$url" >/dev/null 2>&1
    have=$(wc -c < "$out" 2>/dev/null | tr -d " " || echo 0)
    echo "$(date +%T) $out: attempt $attempt -> $have / $want"
  done
  return 1
}
fetch "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf" "qwen2.5-0.5b-instruct-q4_k_m.gguf"
fetch "https://huggingface.co/Qwen/Qwen2.5-Coder-3B-Instruct-GGUF/resolve/main/qwen2.5-coder-3b-instruct-q4_k_m.gguf" "qwen2.5-coder-3b-instruct-q4_k_m.gguf"
echo "$(date +%T) ALL DONE"
