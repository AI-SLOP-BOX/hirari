#!/bin/sh
set -eu

# Reproducible three-hour music pipeline. Chis-A remains an external renderer;
# the generated WAV is treated as an explicit, auditable input to the mix.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${AURA_THREE_HOUR_OUT:-$ROOT_DIR/dist/aura_three_hour_release}
mkdir -p "$OUT_DIR"
OUT_DIR=$(CDPATH= cd -- "$OUT_DIR" && pwd)
cd "$ROOT_DIR"

INPUT_MIDI=${AURA_THREE_HOUR_MIDI:-}
INPUT_JSON=${AURA_THREE_HOUR_MIDI_JSON:-}
if [ -n "$INPUT_MIDI" ] || [ -n "$INPUT_JSON" ]; then
  if [ -z "$INPUT_MIDI" ] || [ -z "$INPUT_JSON" ]; then
    echo "AURA_THREE_HOUR_MIDI and AURA_THREE_HOUR_MIDI_JSON must be provided together" >&2
    exit 2
  fi
  GENERATED_MIDI=$(CDPATH= cd -- "$(dirname -- "$INPUT_MIDI")" && pwd)/$(basename "$INPUT_MIDI")
  GENERATED_JSON=$(CDPATH= cd -- "$(dirname -- "$INPUT_JSON")" && pwd)/$(basename "$INPUT_JSON")
else
  python3 scripts/generate_three_hour_song.py
  GENERATED_MIDI="$ROOT_DIR/dist/aura_three_hour_release/aura_three_hour_chisa.mid"
  GENERATED_JSON="$ROOT_DIR/dist/aura_three_hour_release/aura_three_hour_chisa.midi.json"
fi
if [ -n "$INPUT_MIDI" ]; then
  CANDIDATE_PROVENANCE=${AURA_CHISA_PROVENANCE:-$OUT_DIR/vocal_render_provenance.json}
  if [ ! -f "$CANDIDATE_PROVENANCE" ]; then
    echo "candidate MIDI requires a preverified AURA_CHISA_PROVENANCE file" >&2
    exit 4
  fi
fi
if [ "$GENERATED_MIDI" != "$OUT_DIR/aura_three_hour_chisa.mid" ]; then
  cp "$GENERATED_MIDI" "$OUT_DIR/aura_three_hour_chisa.mid"
fi
if [ "$GENERATED_JSON" != "$OUT_DIR/aura_three_hour_chisa.midi.json" ]; then
  cp "$GENERATED_JSON" "$OUT_DIR/aura_three_hour_chisa.midi.json"
fi

SURGE_RENDERER=${AURA_SURGE_RENDERER:-/tmp/aura_surge_render}
if [ ! -x "$SURGE_RENDERER" ]; then
  echo "missing executable Surge renderer: $SURGE_RENDERER" >&2
  exit 2
fi
"$SURGE_RENDERER" \
  "$OUT_DIR/aura_three_hour_chisa.mid" \
  "$OUT_DIR/aura_three_hour_surge.wav"

VOCAL=${AURA_CHISA_VOCAL:-$OUT_DIR/aura_three_hour_chisa_vocal.wav}
if [ ! -f "$VOCAL" ]; then
  echo "missing Chis-A render: $VOCAL" >&2
  echo "Render the MIDI with Chis-A, then rerun this script." >&2
  exit 3
fi
LOCAL_VOCAL="$OUT_DIR/aura_three_hour_chisa_vocal.wav"
if [ "$VOCAL" != "$LOCAL_VOCAL" ]; then
  cp "$VOCAL" "$LOCAL_VOCAL"
  VOCAL="$LOCAL_VOCAL"
fi
SURGE_SOURCE="$OUT_DIR/aura_three_hour_surge.wav"
SURGE_DURATION=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$SURGE_SOURCE")
VOCAL_DURATION=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$VOCAL")
SURGE_EXTENDED="$OUT_DIR/aura_three_hour_surge_extended.wav"
if python3 - "$VOCAL_DURATION" "$SURGE_DURATION" <<'PY'
import sys
raise SystemExit(0 if float(sys.argv[1]) > float(sys.argv[2]) + 0.05 else 1)
PY
then
  EXTEND_SECONDS=$(python3 - "$VOCAL_DURATION" "$SURGE_DURATION" <<'PY'
import sys
print(max(0.0, float(sys.argv[1]) - float(sys.argv[2])))
PY
  )
  TAIL_WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aura-tail.XXXXXX")
  trap 'rm -rf "$TAIL_WORK_DIR"' EXIT
  TAIL_START_BEAT=$(python3 - "$SURGE_DURATION" <<'PY'
import sys
print(float(sys.argv[1]) * 110.0 / 60.0)
PY
  )
  TAIL_BUILD_JSON=$(python3 scripts/build_tail_midi.py \
    "$OUT_DIR/aura_three_hour_chisa.midi.json" "$TAIL_WORK_DIR/tail.mid" "$TAIL_START_BEAT")
  python3 - "$SURGE_DURATION" "$VOCAL_DURATION" "$TAIL_START_BEAT" "$EXTEND_SECONDS" "$TAIL_BUILD_JSON" "$OUT_DIR/tail_render_manifest.json" <<'PY'
import json
import sys
from pathlib import Path

manifest = {
    "method": "midi_tail_render_crossfade",
    "raw_surge_seconds": float(sys.argv[1]),
    "vocal_seconds": float(sys.argv[2]),
    "tail_start_beat": float(sys.argv[3]),
    "extension_seconds": float(sys.argv[4]),
    "tail_builder": json.loads(sys.argv[5]),
}
Path(sys.argv[6]).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
  "$SURGE_RENDERER" "$TAIL_WORK_DIR/tail.mid" "$TAIL_WORK_DIR/tail.wav"
  ffmpeg -y -i "$TAIL_WORK_DIR/tail.wav" -t "$EXTEND_SECONDS" \
    -ar 44100 -ac 2 -c:a pcm_s24le "$TAIL_WORK_DIR/tail_trimmed.wav" >/dev/null 2>&1
  ffmpeg -y -i "$SURGE_SOURCE" -i "$TAIL_WORK_DIR/tail_trimmed.wav" -filter_complex \
    "[0:a]asetpts=PTS-STARTPTS[head];[1:a]asetpts=PTS-STARTPTS[tail];[head][tail]acrossfade=d=0.08:c1=tri:c2=tri[out]" \
    -map "[out]" -ar 44100 -ac 2 -c:a pcm_s24le "$SURGE_EXTENDED" >/dev/null 2>&1
else
  cp "$SURGE_SOURCE" "$SURGE_EXTENDED"
  python3 - "$SURGE_DURATION" "$VOCAL_DURATION" "$OUT_DIR/tail_render_manifest.json" <<'PY'
import json
import sys
from pathlib import Path
Path(sys.argv[3]).write_text(json.dumps({
    "method": "raw_surge_copy",
    "raw_surge_seconds": float(sys.argv[1]),
    "vocal_seconds": float(sys.argv[2]),
    "tail_start_beat": None,
    "extension_seconds": 0.0,
}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
fi
PROVENANCE="$OUT_DIR/vocal_render_provenance.json"
PROVENANCE_SOURCE=${AURA_CHISA_PROVENANCE:-$PROVENANCE}
PROVENANCE_READY=0
if [ -f "$PROVENANCE_SOURCE" ]; then
  python3 scripts/check_vocal_provenance.py \
    "$OUT_DIR/aura_three_hour_chisa.midi.json" "$VOCAL" \
    --verify "$PROVENANCE_SOURCE" >/dev/null
  if [ "$PROVENANCE_SOURCE" != "$PROVENANCE" ]; then
    cp "$PROVENANCE_SOURCE" "$PROVENANCE"
  fi
  PROVENANCE_READY=1
fi

DURATION=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$VOCAL")
FADE_START=$(python3 -c 'import sys; print(max(0.0, float(sys.argv[1]) - 4.0))' "$DURATION")
ffmpeg -y \
  -i "$SURGE_EXTENDED" \
  -i "$VOCAL" \
  -filter_complex "[0:a]apad=whole_dur=$DURATION,atrim=duration=$DURATION,volume=0.74,acompressor=threshold=-18dB:ratio=2.4:attack=12:release=180:makeup=2[inst];[1:a]atrim=duration=$DURATION,highpass=f=80,lowpass=f=15000,acompressor=threshold=-28dB:ratio=3.2:attack=8:release=140:makeup=6,volume=2.5[voc];[1:a]atrim=duration=$DURATION,highpass=f=120,lowpass=f=9000,chorus=0.5:0.7:45:0.25:0.35:2,volume=0.14[support];[inst][voc][support]amix=inputs=3:duration=longest:dropout_transition=2,afade=t=out:st=$FADE_START:d=4,loudnorm=I=-14:TP=-1:LRA=8[out]" \
  -map "[out]" -ar 44100 -ac 2 -c:a pcm_s24le "$OUT_DIR/aura_three_hour_mix.wav" >/dev/null 2>&1
ffmpeg -y -i "$OUT_DIR/aura_three_hour_mix.wav" -c:a pcm_s16le "$OUT_DIR/aura_three_hour_mix_cli.wav" >/dev/null 2>&1

cargo build -q -p aura-core-bridge --bin aura
target/debug/aura ci verify "$OUT_DIR/aura_three_hour_mix_cli.wav" 0.99 0.0001 > "$OUT_DIR/audio_verify.json"
python3 scripts/check_song_quality.py \
  "$OUT_DIR/aura_three_hour_chisa.midi.json" > "$OUT_DIR/song_quality.json"
python3 scripts/export_release_lyrics.py \
  "$OUT_DIR/aura_three_hour_chisa.midi.json" "$OUT_DIR/lyrics.txt" > /dev/null
python3 scripts/check_audio_joins.py \
  "$SURGE_EXTENDED" "$OUT_DIR/join_quality.json" "$SURGE_DURATION" > /dev/null
python3 scripts/analyze_three_hour_mix.py \
  "$VOCAL" \
  "$SURGE_EXTENDED" \
  "$OUT_DIR/aura_three_hour_mix_cli.wav" \
  "$OUT_DIR/mix_quality.json" > /dev/null
python3 scripts/build_cli_song_project.py \
  "$OUT_DIR/aura_three_hour_chisa.midi.json" \
  "$OUT_DIR/aura_three_hour_song.aura" >/dev/null
target/debug/aura project inspect "$OUT_DIR/aura_three_hour_song.aura" > "$OUT_DIR/project_inspect.json"
target/debug/aura project manifest "$OUT_DIR/aura_three_hour_song.aura" > "$OUT_DIR/project_manifest.json"
cp "$ROOT_DIR/scripts/check_vocal_provenance.py" "$OUT_DIR/check_vocal_provenance.py"
cp "$ROOT_DIR/scripts/build_tail_midi.py" "$OUT_DIR/build_tail_midi.py"
cp "$ROOT_DIR/scripts/export_release_lyrics.py" "$OUT_DIR/export_release_lyrics.py"
cp "$ROOT_DIR/docs/three_hour_remaining_work.md" "$OUT_DIR/remaining_work.md"
cp "$ROOT_DIR/scripts/generate_three_hour_song.py" "$OUT_DIR/generate_three_hour_song.py"
cp "$ROOT_DIR/scripts/check_song_quality.py" "$OUT_DIR/check_song_quality.py"
cp "$ROOT_DIR/scripts/build_cli_song_project.py" "$OUT_DIR/build_cli_song_project.py"
cp "$ROOT_DIR/scripts/analyze_three_hour_mix.py" "$OUT_DIR/analyze_three_hour_mix.py"
cp "$ROOT_DIR/scripts/audit_three_hour_release.py" "$OUT_DIR/audit_three_hour_release.py"
cp "$ROOT_DIR/scripts/run_three_hour_release.sh" "$OUT_DIR/run_three_hour_release.sh"
cp "$ROOT_DIR/scripts/propose_mora_grouping.py" "$OUT_DIR/propose_mora_grouping.py"
cp "$ROOT_DIR/scripts/propose_outro_hook.py" "$OUT_DIR/propose_outro_hook.py"
cp "$ROOT_DIR/scripts/json_to_midi_candidate.py" "$OUT_DIR/json_to_midi_candidate.py"
cp "$ROOT_DIR/scripts/build_lyric_candidate.py" "$OUT_DIR/build_lyric_candidate.py"
cp "$ROOT_DIR/scripts/build_arrangement_candidate.py" "$OUT_DIR/build_arrangement_candidate.py"
cp "$ROOT_DIR/scripts/build_vocal_harmony_candidate.py" "$OUT_DIR/build_vocal_harmony_candidate.py"
cp "$ROOT_DIR/scripts/json_to_ust_candidate.py" "$OUT_DIR/json_to_ust_candidate.py"
cp "$ROOT_DIR/scripts/check_audio_joins.py" "$OUT_DIR/check_audio_joins.py"
CANONICAL_RELEASE="$ROOT_DIR/dist/aura_three_hour_release"
if [ "$OUT_DIR" != "$CANONICAL_RELEASE" ]; then
  cp "$CANONICAL_RELEASE/README.md" "$OUT_DIR/README.md"
  cp "$CANONICAL_RELEASE/production_notes.md" "$OUT_DIR/production_notes.md"
fi
if [ "$PROVENANCE_READY" -eq 0 ]; then
  python3 scripts/check_vocal_provenance.py \
    "$OUT_DIR/aura_three_hour_chisa.midi.json" "$VOCAL" \
    --write "$PROVENANCE" >/dev/null
fi
python3 scripts/audit_three_hour_release.py "$OUT_DIR" > /dev/null
# Keep the release directory portable: recovery backups are useful during an
# interactive save, but are not part of the published song bundle.
find "$OUT_DIR" -maxdepth 1 -type f -name 'aura_three_hour_song.aura.bak.*' -delete
date '+%Y-%m-%d %H:%M:%S %Z' > "$OUT_DIR/completed_at.txt"
echo "three-hour release completed: $OUT_DIR"
