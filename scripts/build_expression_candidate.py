#!/usr/bin/env python3
"""Shape vocal timing, dynamics, and small melodic inflections for audition."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: build_expression_candidate.py INPUT_JSON OUTPUT_JSON OUTPUT_MID")
    source, output_json, output_mid = map(Path, sys.argv[1:])
    document = json.loads(source.read_text(encoding="utf-8"))
    candidate = copy.deepcopy(document)
    vocal = next(track for track in candidate if int(track["track_id"]) == 3)
    notes = sorted(vocal["notes"], key=lambda item: float(item["start_beat"]))
    groups: list[list[dict]] = []
    for note in notes:
        if not groups or float(note["start_beat"]) - float(groups[-1][-1]["start_beat"]) > 1.5:
            groups.append([])
        groups[-1].append(note)
    offsets = (0, 0, 1, 0, -1, 0, 0, 1, 0, -1, 0, 0)
    for phrase_index, group in enumerate(groups):
        for position, note in enumerate(group):
            # Leave the written timing grid intact; only shape note gates.
            if position == 0:
                gate = 0.38
            elif position == len(group) - 1:
                gate = 0.92
            elif position == len(group) - 2:
                gate = 0.42
            else:
                gate = 0.46 if position % 5 == 4 else 0.48
            note["length_beats"] = min(float(note["length_beats"]), gate)
            base = 88 if phrase_index < 8 else 98
            swell = 16 if position in (2, 3, 4) else 7 if position % 4 == 0 else 0
            cadence = -9 if position == len(group) - 1 else 0
            note["velocity"] = max(58, min(118, base + swell + cadence))
            note["pitch"] = max(36, min(96, int(note["pitch"]) + offsets[(position + phrase_index) % len(offsets)]))
    vocal["notes"] = notes
    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(json.dumps(candidate, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    subprocess.run([sys.executable, str(Path(__file__).with_name("json_to_midi_candidate.py")),
                    str(output_json), str(output_mid)], check=True)
    print(json.dumps({"ok": True, "phrases": len(groups), "notes": len(notes),
                      "output": str(output_json)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
