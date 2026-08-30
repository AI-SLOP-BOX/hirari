#!/bin/zsh
set -euo pipefail

ROOT="/Users/REDACTED/Library/OpenUtau/Singers/KasaneTetoOfficial/重音テト単独音"
INSTRUMENTAL="${AURA_TETO_INSTRUMENTAL:-/Users/REDACTED/Documents/aura_full_song_preview.wav}"
VOCAL="${AURA_TETO_VOCAL:-/Users/REDACTED/Documents/aura_teto_vocal.wav}"
MIX="${AURA_TETO_MIX:-/Users/REDACTED/Documents/aura_teto_song_preview.wav}"
WORK="$(mktemp -d /tmp/aura-teto.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

typeset -a lyrics=(ひ か り ほ ど け る よ る に き み と み つ け た こ え)
typeset -a notes=(60 62 64 64 62 60 62 64 65 64 67 65 64 62 60 62 64 65 64)
typeset -a durations=(0.48 0.48 0.48 0.48 0.48 0.48 0.48 0.48 0.48 0.48 0.96 0.48 0.48 0.48 0.48 0.48 0.48 0.48 0.96)

function make_note() {
  local lyric="$1" midi="$2" duration="$3" out="$4"
  local src="$ROOT/_$lyric.wav"
  local factor="$(python3 - "$midi" <<'PY'
import math, sys
print(f"{2 ** ((int(sys.argv[1]) - 60) / 12):.10f}")
PY
)"
  ffmpeg -hide_banner -loglevel error -y -i "$src" \
    -af "asetrate=44100*$factor,aresample=44100,atempo=$(python3 - "$factor" <<'PY'
import sys
print(f"{1 / float(sys.argv[1]):.10f}")
PY
),atrim=0:$duration,apad,atrim=0:$duration,afade=t=in:st=0:d=0.015,afade=t=out:st=$(python3 - "$duration" <<'PY'
import sys
print(max(0.01, float(sys.argv[1]) - 0.045))
PY
):d=0.045" \
    -ar 44100 -ac 1 -c:a pcm_s16le "$out"
}

idx=0
list="$WORK/concat.txt"
: > "$list"
for i in {1..3}; do
  for ((j=1; j<=${#lyrics}; j++)); do
    idx=$((idx + 1))
    out="$WORK/note_${idx}.wav"
    make_note "${lyrics[$j]}" "${notes[$j]}" "${durations[$j]}" "$out"
    print -r -- "file '$out'" >> "$list"
  done
done

ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i "$list" \
  -ar 44100 -ac 1 -c:a pcm_s16le "$WORK/phrase.wav"

# Place three chorus passes over the 96-bar instrumental: bars 17, 49, and 81.
ffmpeg -hide_banner -loglevel error -y -i "$WORK/phrase.wav" \
  -filter_complex "[0:a]asplit=3[s1][s2][s3];[s1]adelay=32000[a];[s2]adelay=96000[b];[s3]adelay=160000[c];[a][b][c]amix=inputs=3:normalize=0,volume=0.85" \
  -ar 44100 -ac 1 -c:a pcm_s16le "$VOCAL"

ffmpeg -hide_banner -loglevel error -y -i "$INSTRUMENTAL" -i "$VOCAL" \
  -filter_complex "[0:a]volume=0.0dB[bed];[1:a]pan=stereo|c0=c0|c1=c0,volume=5.5dB[v];[bed][v]amix=inputs=2:duration=first:normalize=0,alimiter=limit=0.95" \
  -ar 44100 -ac 2 -c:a pcm_s16le "$MIX"

echo "$VOCAL"
echo "$MIX"
