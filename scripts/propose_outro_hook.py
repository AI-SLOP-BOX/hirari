#!/usr/bin/env python3
"""Add a restrained final hook to a temporary three-hour-song candidate."""

import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: propose_outro_hook.py INPUT_JSON OUTPUT_JSON")
    source = Path(sys.argv[1])
    target = Path(sys.argv[2])
    tracks = json.loads(source.read_text(encoding="utf-8"))
    hook = "あたらしいくつであるきだす"
    contour = [72, 74, 76, 74, 72, 70, 69, 67, 65, 67, 69, 70, 72, 74]
    vocal = next(track for track in tracks if track.get("track_id") == 3)
    notes = list(vocal.get("notes", []))
    start = max(float(note["start_beat"]) + float(note["length_beats"]) for note in notes)
    step = 0.5
    for index, (lyric, pitch) in enumerate(zip(hook, contour)):
        notes.append({
            "pitch": pitch,
            "start_beat": round(start + index * step, 4),
            "length_beats": 1.0 if index == len(hook) - 1 else 0.4625,
            "velocity": 108,
            "lyric": lyric,
        })
    vocal["notes"] = sorted(notes, key=lambda item: float(item["start_beat"]))
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(tracks, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"ok": True, "added_notes": len(hook), "hook": hook, "output": str(target)}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
