#!/usr/bin/env bash
set -euo pipefail

# Reproducible Hirari-native session pass. Unlike the external song pipeline,
# this path puts the rendered assets into Hirari regions and asks Hirari's own
# offline engine to produce the mix and stems.
ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
OUT_DIR=${1:?usage: run_native_session_release.sh OUTPUT_DIR}
SOURCE_HIRARI=${2:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_HIRARI VOCAL_WAV INSTRUMENT_WAV}
VOCAL_WAV=${3:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_HIRARI VOCAL_WAV INSTRUMENT_WAV}
INSTRUMENT_WAV=${4:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_HIRARI VOCAL_WAV INSTRUMENT_WAV}

mkdir -p "$OUT_DIR"
PROJECT="$OUT_DIR/hirari_native_session.hirari"
MIX="$OUT_DIR/hirari_native_session_bounce.wav"
STEMS="$OUT_DIR/stems"
python3 "$ROOT_DIR/scripts/build_hirari_native_session.py" \
  "$SOURCE_HIRARI" "$VOCAL_WAV" "$INSTRUMENT_WAV" "$PROJECT"
"$ROOT_DIR/target/debug/hirari" project inspect "$PROJECT" > "$OUT_DIR/project_inspect.json"
"$ROOT_DIR/target/debug/hirari" project manifest "$PROJECT" > "$OUT_DIR/project_manifest.json"
"$ROOT_DIR/target/debug/hirari" render mix "$PROJECT" "$MIX" wav > "$OUT_DIR/mix_render.json"
"$ROOT_DIR/target/debug/hirari" ci verify "$MIX" 0.99 0.0001 > "$OUT_DIR/mix_verify.json"
"$ROOT_DIR/target/debug/hirari" render stems "$PROJECT" "$STEMS" wav > "$OUT_DIR/stems_render.json"
for stem in "$STEMS"/*.wav; do
  name=$(basename "$stem" .wav)
  "$ROOT_DIR/target/debug/hirari" ci verify "$stem" 0.99 0.0001 > "$OUT_DIR/${name}.verify.json"
done
echo "Hirari native session release passed: $OUT_DIR"
