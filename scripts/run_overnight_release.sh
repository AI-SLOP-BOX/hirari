#!/bin/sh
set -eu

# Reproducible overnight music pipeline. Chis-A remains an external renderer;
# the generated WAV is treated as an explicit, auditable input to the mix.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${AURA_OVERNIGHT_OUT:-$ROOT_DIR/dist/aura_overnight_release}
mkdir -p "$OUT_DIR"
cd "$ROOT_DIR"

python3 scripts/generate_overnight_song.py
cp dist/aura_final_chisa/aura_final_chisa.mid "$OUT_DIR/aura_final_chisa.mid"
cp dist/aura_final_chisa/aura_final_chisa.midi.json "$OUT_DIR/aura_final_chisa.midi.json"

SURGE_RENDERER=${AURA_SURGE_RENDERER:-/tmp/aura_surge_render}
if [ ! -x "$SURGE_RENDERER" ]; then
  echo "missing executable Surge renderer: $SURGE_RENDERER" >&2
  exit 2
fi
"$SURGE_RENDERER" \
  "$OUT_DIR/aura_final_chisa.mid" \
  "$OUT_DIR/aura_final_surge.wav"

VOCAL=${AURA_CHISA_VOCAL:-$OUT_DIR/aura_final_chisa_vocal.wav}
if [ ! -f "$VOCAL" ]; then
  echo "missing Chis-A render: $VOCAL" >&2
  echo "Render the MIDI with Chis-A, then rerun this script." >&2
  exit 3
fi

DURATION=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$VOCAL")
ffmpeg -y \
  -i "$OUT_DIR/aura_final_surge.wav" \
  -i "$VOCAL" \
  -filter_complex "[0:a]atrim=duration=$DURATION,volume=0.78,acompressor=threshold=-18dB:ratio=2.2:attack=12:release=180:makeup=2[inst];[1:a]atrim=duration=$DURATION,highpass=f=80,lowpass=f=15000,acompressor=threshold=-27dB:ratio=3.0:attack=8:release=140:makeup=6,volume=1.6[voc];[inst][voc]amix=inputs=2:duration=first:dropout_transition=2,loudnorm=I=-14:TP=-1:LRA=8[out]" \
  -map "[out]" -ar 44100 -ac 2 -c:a pcm_s24le "$OUT_DIR/aura_final_mix.wav" >/dev/null 2>&1
ffmpeg -y -i "$OUT_DIR/aura_final_mix.wav" -c:a pcm_s16le "$OUT_DIR/aura_final_mix_cli.wav" >/dev/null 2>&1

cargo build -q -p aura-core-bridge --bin aura
target/debug/aura ci verify "$OUT_DIR/aura_final_mix_cli.wav" 0.99 0.0001 > "$OUT_DIR/audio_verify.json"
python3 scripts/build_cli_song_project.py \
  "$OUT_DIR/aura_final_chisa.midi.json" \
  "$OUT_DIR/aura_overnight_song.aura" >/dev/null
target/debug/aura project inspect "$OUT_DIR/aura_overnight_song.aura" > "$OUT_DIR/project_inspect.json"
target/debug/aura project manifest "$OUT_DIR/aura_overnight_song.aura" > "$OUT_DIR/project_manifest.json"
date '+%Y-%m-%d %H:%M:%S %Z' > "$OUT_DIR/completed_at.txt"
echo "overnight release completed: $OUT_DIR"
