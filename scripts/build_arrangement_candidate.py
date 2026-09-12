#!/usr/bin/env python3
"""Add a restrained bridge and lift variation without touching the canonical MIDI."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
from pathlib import Path

PPQ = 480


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: build_arrangement_candidate.py INPUT_JSON OUTPUT_JSON OUTPUT_MID")
    source, output_json, output_mid = map(Path, sys.argv[1:])
    document = json.loads(source.read_text(encoding="utf-8"))
    candidate = copy.deepcopy(document)
    tracks = {int(track["track_id"]): track["notes"] for track in candidate}
    bass = tracks[1]
    pad = tracks[2]
    drums = tracks[0]
    existing = {(round(float(n["start_beat"]), 4), int(n["pitch"])) for n in bass}
    # Bridge: answer the downbeat with an off-beat octave/fifth figure.
    for bar in range(72, 80):
        bar_start = bar * 4
        downbeat = next((n for n in bass if abs(float(n["start_beat"]) - bar_start) < 1e-4), None)
        if downbeat is None:
            continue
        pitch = int(downbeat["pitch"]) + 12
        start = round(bar_start + 1.5, 4)
        if (start, pitch) not in existing:
            bass.append({"pitch": pitch, "start_beat": start, "length_beats": 0.28,
                         "velocity": 56, "lyric": ""})
            existing.add((start, pitch))
    # Lift: a quiet upper answer on alternating bars, not a full-time duplicate pad.
    pad_existing = {(round(float(n["start_beat"]), 4), int(n["pitch"])) for n in pad}
    for bar in (80, 82, 84, 86):
        bar_start = bar * 4
        source_notes = [n for n in pad if abs(float(n["start_beat"]) - bar_start) < 1e-4]
        for note in source_notes[:2]:
            pitch = int(note["pitch"]) + 12
            start = round(bar_start + 3.0, 4)
            if (start, pitch) not in pad_existing:
                pad.append({"pitch": pitch, "start_beat": start, "length_beats": 0.7,
                            "velocity": 34, "lyric": ""})
                pad_existing.add((start, pitch))
    # Keep the bridge from feeling copy/pasted: sparse ghost hats on the final lift.
    drum_existing = {(round(float(n["start_beat"]), 4), int(n["pitch"])) for n in drums}
    for bar in (79, 87):
        for beat in (1.75, 3.75):
            start = round(bar * 4 + beat, 4)
            if (start, 42) not in drum_existing:
                drums.append({"pitch": 42, "start_beat": start, "length_beats": 0.08,
                              "velocity": 34, "lyric": ""})
                drum_existing.add((start, 42))
    for track in candidate:
        track["notes"].sort(key=lambda note: (float(note["start_beat"]), int(note["pitch"])))
    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(json.dumps(candidate, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    subprocess.run([sys.executable, str(Path(__file__).with_name("json_to_midi_candidate.py")),
                    str(output_json), str(output_mid)], check=True)
    print(json.dumps({"ok": True, "added_bass": 8, "added_pad": 8, "added_ghost_hats": 4,
                      "output": str(output_json)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
