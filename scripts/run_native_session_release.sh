#!/usr/bin/env bash
set -euo pipefail

# Reproducible Aura-native session pass. Unlike the external song pipeline,
# this path puts the rendered assets into Aura regions and asks Aura's own
# offline engine to produce the mix and stems.
ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
OUT_DIR=${1:?usage: run_native_session_release.sh OUTPUT_DIR}
SOURCE_AURA=${2:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_AURA VOCAL_WAV INSTRUMENT_WAV}
VOCAL_WAV=${3:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_AURA VOCAL_WAV INSTRUMENT_WAV}
INSTRUMENT_WAV=${4:?usage: run_native_session_release.sh OUTPUT_DIR SOURCE_AURA VOCAL_WAV INSTRUMENT_WAV}

mkdir -p "$OUT_DIR"
PROJECT="$OUT_DIR/aura_native_session.aura"
MIX="$OUT_DIR/aura_native_session_bounce.wav"
STEMS="$OUT_DIR/stems"
python3 "$ROOT_DIR/scripts/build_aura_native_session.py" \
  "$SOURCE_AURA" "$VOCAL_WAV" "$INSTRUMENT_WAV" "$PROJECT"
"$ROOT_DIR/target/debug/aura" project inspect "$PROJECT" > "$OUT_DIR/project_inspect.json"
"$ROOT_DIR/target/debug/aura" project manifest "$PROJECT" > "$OUT_DIR/project_manifest.json"
"$ROOT_DIR/target/debug/aura" render mix "$PROJECT" "$MIX" wav > "$OUT_DIR/mix_render.json"
"$ROOT_DIR/target/debug/aura" ci verify "$MIX" 0.99 0.0001 > "$OUT_DIR/mix_verify.json"
"$ROOT_DIR/target/debug/aura" render stems "$PROJECT" "$STEMS" wav > "$OUT_DIR/stems_render.json"
for stem in "$STEMS"/*.wav; do
  name=$(basename "$stem" .wav)
  "$ROOT_DIR/target/debug/aura" ci verify "$stem" 0.99 0.0001 > "$OUT_DIR/${name}.verify.json"
done
echo "Aura native session release passed: $OUT_DIR"
