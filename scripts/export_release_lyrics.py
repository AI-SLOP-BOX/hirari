#!/usr/bin/env python3
"""Export the canonical vocal lyric stream as readable phrase lines."""

import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: export_release_lyrics.py MIDI_JSON OUTPUT_TXT")
    tracks = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    vocal = next(track["notes"] for track in tracks if int(track["track_id"]) == 3)
    ordered = sorted(vocal, key=lambda note: float(note["start_beat"]))
    phrases = []
    current = []
    previous = None
    for note in ordered:
        start = float(note["start_beat"])
        if previous is not None and start - previous > 1.5 and current:
            phrases.append("".join(current))
            current = []
        token = str(note.get("lyric", ""))
        if token:
            current.append(token)
        previous = start
    if current:
        phrases.append("".join(current))
    Path(sys.argv[2]).write_text("\n".join(phrases) + "\n", encoding="utf-8")
    print(json.dumps({"ok": True, "phrases": len(phrases), "output": sys.argv[2]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
