#!/usr/bin/env python3
"""Check extension joins for discontinuities large enough to click."""

import json
import math
import struct
import subprocess
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) not in (3, 4):
        raise SystemExit("usage: check_audio_joins.py AUDIO OUT_JSON [JOIN_SECONDS]")
    audio, output = map(Path, sys.argv[1:3])
    join_seconds = float(sys.argv[3]) if len(sys.argv) == 4 else 192.0
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(audio), "-f", "f32le", "-ac", "2", "-ar", "44100", "pipe:1"],
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    values = struct.unpack("<" + "f" * (len(raw) // 4), raw)
    channels = 2
    joins = {}
    points = (max(0.0, join_seconds - 24.0), join_seconds)
    for seconds in points:
        center = int(seconds * 44100) * channels
        window = values[max(channels, center - channels): center + channels]
        jumps = [abs(window[index] - window[index - channels]) for index in range(channels, len(window))]
        joins[str(round(seconds, 3))] = round(max(jumps, default=0.0), 8)
    # A short crossfade intentionally changes the waveform slope at the join;
    # 0.05 is well below an audible hard splice while allowing that slope.
    threshold = 0.05
    result = {"ok": all(value <= threshold for value in joins.values()), "normalized_sample_jump": joins, "threshold": threshold}
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
