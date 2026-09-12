#!/usr/bin/env python3
"""Build a pronunciation candidate without changing the signed release.

Japanese small vowels and y-kana are part of the preceding mora for singing.
This tool merges only those tokens; sokuon (っ) stays independent because it
represents a timed consonant closure. The output is deliberately a candidate:
it must be rendered by the external Chis-A/VoiSona path before adoption.
"""

import json
import sys
from pathlib import Path


MERGEABLE = set("ゃゅょぁぃぅぇぉゎ")


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: propose_mora_grouping.py INPUT_JSON OUTPUT_JSON")
    source = Path(sys.argv[1])
    target = Path(sys.argv[2])
    tracks = json.loads(source.read_text(encoding="utf-8"))
    changed = 0
    for track in tracks:
        if track.get("track_id") != 3:
            continue
        notes = sorted(track.get("notes", []), key=lambda item: float(item["start_beat"]))
        kept = []
        for note in notes:
            token = str(note.get("lyric", ""))
            if token in MERGEABLE and kept:
                previous = kept[-1]
                previous_end = float(previous["start_beat"]) + float(previous["length_beats"])
                note_end = float(note["start_beat"]) + float(note["length_beats"])
                previous["lyric"] = str(previous.get("lyric", "")) + token
                previous["length_beats"] = round(max(previous_end, note_end) - float(previous["start_beat"]), 4)
                changed += 1
                continue
            kept.append(dict(note))
        track["notes"] = kept
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(tracks, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"ok": True, "merged_mora_tokens": changed, "output": str(target)}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
