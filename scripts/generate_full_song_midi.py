#!/usr/bin/env python3
"""Create a longer, sectioned Aura MIDI arrangement for real-device testing."""

import json
import sys
from pathlib import Path


BEATS_PER_BAR = 4.0
TOTAL_BARS = 96
CHORDS = [(57, 60, 64), (53, 57, 60), (48, 52, 55), (55, 59, 62)]  # Am F C G


def n(pitch, start, length, velocity, articulation=0):
    return {
        "pitch": pitch,
        "start_beat": round(start, 4),
        "length_beats": round(length, 4),
        "velocity": velocity,
        "articulation": articulation,
    }


def add_drums(out, start, section, bar_in_section):
    if section == "intro":
        if bar_in_section in (4, 6):
            out.append(n(42, start + 3.0, 0.12, 48))
        return
    if section == "break":
        if bar_in_section % 2 == 0:
            out.append(n(36, start, 0.22, 72))
        return
    kick_beats = (0.0, 1.5, 2.0, 3.25) if section in ("chorus", "final") else (0.0, 2.0)
    for beat in kick_beats:
        out.append(n(36, start + beat, 0.2, 108 if section in ("chorus", "final") else 92))
    for beat in (1.0, 3.0):
        out.append(n(38, start + beat, 0.2, 104))
    step = 0.5 if section in ("chorus", "final") else 1.0
    beat = 0.0
    while beat < 4.0:
        out.append(n(42, start + beat, 0.1, 68 if beat % 1 else 78))
        beat += step
    if bar_in_section == 15:
        out.extend((n(45, start + 3.0, 0.18, 82), n(49, start + 3.5, 0.2, 96)))


def make_song():
    tracks = {0: [], 1: [], 2: [], 3: []}
    sections = (["intro"] * 8 + ["verse"] * 16 + ["chorus"] * 16 +
                ["break"] * 8 + ["chorus"] * 24 + ["bridge"] * 16 + ["final"] * 8)
    for bar, section in enumerate(sections):
        start = bar * BEATS_PER_BAR
        bar_in_section = bar % (8 if section in ("intro", "break", "final") else 16)
        chord = CHORDS[bar % len(CHORDS)]
        add_drums(tracks[0], start, section, bar_in_section)

        if section != "intro" and section != "break":
            root = chord[0] - 12
            pattern = (0.0, 1.5, 2.0, 3.0) if section in ("chorus", "final") else (0.0, 2.0)
            for beat in pattern:
                octave = 12 if beat == 2.0 and section in ("chorus", "final") else 0
                tracks[1].append(n(root + octave, start + beat, 0.72, 98 if section in ("chorus", "final") else 82))

        if section == "intro":
            # A simple broken-chord figure establishes the key before drums.
            for index, pitch in enumerate((chord[0] + 12, chord[1] + 12, chord[2] + 12, chord[1] + 12)):
                tracks[2].append(n(pitch, start + index, 0.72, 62))
        elif section in ("verse", "bridge"):
            motif = (chord[2], chord[1], chord[0] + 12, chord[1])
            for index, pitch in enumerate(motif):
                tracks[2].append(n(pitch, start + index, 0.55, 78 if section == "verse" else 84))
        elif section in ("chorus", "final"):
            motif = (chord[2] + 12, chord[1] + 12, chord[0] + 24, chord[1] + 12,
                     chord[2] + 12, chord[1] + 12, chord[0] + 12, chord[1] + 12)
            for index, pitch in enumerate(motif):
                tracks[2].append(n(pitch, start + index * 0.5, 0.36, 108))

        if section != "break":
            pad_velocity = 82 if section in ("chorus", "final") else 64
            for pitch in chord:
                tracks[3].append(n(pitch, start, 3.8, pad_velocity))
            if section in ("chorus", "final"):
                tracks[3].append(n(chord[2] + 12, start + 2.0, 1.8, 70))

    return [{"track_id": track_id, "notes": notes} for track_id, notes in tracks.items()]


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: generate_full_song_midi.py PROJECT.aura")
    project = Path(sys.argv[1]).expanduser().resolve()
    sidecar = Path(f"{project}.midi.json")
    song = make_song()
    sidecar.write_text(json.dumps(song, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {sidecar} ({sum(len(t['notes']) for t in song)} notes, {TOTAL_BARS} bars)")


if __name__ == "__main__":
    main()
