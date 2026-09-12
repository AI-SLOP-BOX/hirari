#!/usr/bin/env python3
"""Build a canonical Aura project from the overnight MIDI sidecar.

The command-line project is intentionally the source of truth for note timing;
the external renderers consume the same MIDI file and the resulting project can
be inspected or rendered by the normal Aura CLI.
"""

import json
import subprocess
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: build_cli_song_project.py MIDI_SIDECAR PROJECT")
    sidecar = Path(sys.argv[1]).expanduser().resolve()
    project = Path(sys.argv[2]).expanduser().resolve()
    project.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        # The release mix is rendered at 44.1 kHz for broad video/distribution
        # compatibility; keep the project timeline at the same rate so the
        # generated sample positions remain directly auditable.
        ["target/debug/aura", "project", "init", str(project), "Aura Overnight", "44100"],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    for name, track_type in (("Drums", "Audio"), ("Bass", "Midi"), ("Harmony", "Midi"), ("Vocal", "Midi")):
        subprocess.run(
            ["target/debug/aura", "track", "add", str(project), name, track_type],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    document = json.loads(project.read_text(encoding="utf-8"))
    tracks = {int(track["id"]): track for track in document["tracks"]}
    source = json.loads(sidecar.read_text(encoding="utf-8"))
    sample_rate = int(document["sample_rate"])
    bpm = 96.0
    track_map = {0: 1, 1: 2, 2: 3, 3: 4}
    notes = []
    for track in source:
        for note in track["notes"]:
            start = int(round(note["start_beat"] * 60.0 / bpm * sample_rate))
            length = max(1, int(round(note["length_beats"] * 60.0 / bpm * sample_rate)))
            notes.append(
                {
                    "track_id": track_map[track["track_id"]],
                    "pitch": int(note["pitch"]),
                    "velocity": int(note["velocity"]),
                    "start_sample": start,
                    "length_samples": length,
                    "lyric": note.get("lyric", ""),
                    "phoneme": note.get("phoneme", ""),
                    "pitch_curve_cents": [],
                    "vibrato_depth_cents": 0,
                    "portamento_samples": 0,
                    "probability": 100,
                    "repeat_count": 1,
                }
            )
    document["midi_notes"] = notes
    document["metadata"]["bpm"] = bpm
    document["metadata"]["tracks_count"] = len(tracks)
    project.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {project} ({len(notes)} notes, {len(tracks)} tracks)")


if __name__ == "__main__":
    main()
