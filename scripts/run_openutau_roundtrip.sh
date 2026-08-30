#!/bin/sh
set -eu

# OpenUtau round-trip evidence runner.  The renderer is deliberately injected
# so CI never pretends that a source-only parser is an OpenUtau integration.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE=${AURA_OPENUTAU_SOURCE:-}
OUTPUT=${AURA_OPENUTAU_RENDER_OUTPUT:-}
RENDER_COMMAND=${AURA_OPENUTAU_RENDER_COMMAND:-}
STRICT=${AURA_OPENUTAU_STRICT:-0}

if [ -z "$SOURCE" ] || [ -z "$OUTPUT" ] || [ -z "$RENDER_COMMAND" ]; then
    message="set AURA_OPENUTAU_SOURCE, AURA_OPENUTAU_RENDER_OUTPUT, and AURA_OPENUTAU_RENDER_COMMAND"
    if [ "$STRICT" = "1" ]; then
        echo "OpenUtau round-trip: FAIL ($message)" >&2
        exit 1
    fi
    echo "OpenUtau round-trip: SKIPPED ($message)"
    exit 0
fi

case "$SOURCE" in *.ust|*.ustx) ;; *) echo "source must be .ust or .ustx" >&2; exit 2 ;; esac
test -f "$SOURCE"
mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT"

# The command receives source/output as positional arguments.  Quoting here
# preserves Unicode paths and spaces while keeping the command user-defined.
sh -c "$RENDER_COMMAND" sh "$SOURCE" "$OUTPUT"
test -s "$OUTPUT"

if command -v ffprobe >/dev/null 2>&1; then
    # A non-empty file is not sufficient evidence: require an actual audio
    # stream and persist its decoded properties for the round-trip report.
    if ! ffprobe -v error -select_streams a:0 -show_entries stream=sample_rate,channels,nb_frames \
        -of json "$OUTPUT" >"$OUTPUT.openutau-audio.json"; then
        echo "OpenUtau round-trip: FAIL (rendered output is not decodable audio)" >&2
        exit 1
    fi
    grep -q '"streams"' "$OUTPUT.openutau-audio.json" || {
        echo "OpenUtau round-trip: FAIL (rendered output has no audio stream)" >&2
        exit 1
    }
elif [ "$STRICT" = "1" ]; then
    echo "OpenUtau round-trip: FAIL (ffprobe is required in strict mode)" >&2
    exit 1
fi

SOURCE_HASH=$(shasum -a 256 "$SOURCE" | awk '{print $1}')
OUTPUT_HASH=$(shasum -a 256 "$OUTPUT" | awk '{print $1}')
printf 'source=%s\noutput=%s\nsource_sha256=%s\noutput_sha256=%s\n' \
    "$SOURCE" "$OUTPUT" "$SOURCE_HASH" "$OUTPUT_HASH"
echo "OpenUtau round-trip: PASS (rendered output is non-empty and auditable)"
