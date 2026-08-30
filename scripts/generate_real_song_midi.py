#!/usr/bin/env python3
"""Generate a deterministic 64-bar Aura MIDI sidecar for real-device smoke runs."""

import json
import sys
from pathlib import Path


BEATS_PER_BAR = 4.0
TOTAL_BARS = 64


def note(pitch, start, length, velocity, articulation=0):
    return {
        "pitch": pitch,
        "start_beat": round(start, 4),
        "length_beats": round(length, 4),
        "velocity": velocity,
        "articulation": articulation,
    }


def make_song():
    tracks = {track_id: [] for track_id in range(4)}
    # Am - F - C - G. Each chord lasts one bar.
    chords = [(57, 60, 64), (53, 57, 60), (48, 52, 55), (55, 59, 62)]

    for bar in range(TOTAL_BARS):
        bar_start = bar * BEATS_PER_BAR
        section_bar = bar % 16
        chord = chords[bar % len(chords)]

        # Kick/snare/hat pattern. The final bar of each 16-bar section opens
        # the hats and adds a small turnaround for a clear arrangement arc.
        for beat in (0.0, 2.0):
            tracks[0].append(note(36, bar_start + beat, 0.25, 112))
        for beat in (1.0, 3.0):
            tracks[0].append(note(38, bar_start + beat, 0.22, 100))
        hat_step = 0.5 if section_bar >= 8 else 1.0
        beat = 0.0
        while beat < 4.0:
            tracks[0].append(note(42, bar_start + beat, 0.12, 58 if beat % 1 else 72))
            beat += hat_step
        if section_bar == 15:
            tracks[0].append(note(49, bar_start + 3.5, 0.25, 92))

        # Bass enters after the intro and follows chord roots with octave
        # movement in the chorus sections.
        if bar >= 4:
            root = chord[0] - (12 if section_bar < 8 else 0)
            for beat in (0.0, 1.5, 2.0, 3.0):
                pitch = root + (12 if beat == 2.0 and section_bar >= 8 else 0)
                tracks[1].append(note(pitch, bar_start + beat, 0.7, 82 if section_bar < 8 else 98))

        # Sustained pad voicing. Leave the first four bars sparse for an intro.
        if bar >= 4:
            for pitch in chord:
                tracks[3].append(note(pitch, bar_start, 3.8, 60 if section_bar < 8 else 76))

        # Lead motif: restrained in verse, octave-up and denser in chorus,
        # absent during the first four-bar break of each 16-bar section.
        if bar >= 8 and section_bar not in (0, 1, 2, 3):
            motif = [chord[2], chord[1], chord[0] + 12, chord[1], chord[2], chord[1], chord[0]]
            step = 0.5 if section_bar >= 8 else 1.0
            for index, pitch in enumerate(motif):
                start = bar_start + index * step
                if start >= bar_start + 4.0:
                    break
                tracks[2].append(note(pitch + (12 if section_bar >= 8 else 0), start, 0.38, 86 if section_bar < 8 else 108))

    return [
        {"track_id": track_id, "notes": notes}
        for track_id, notes in tracks.items()
    ]


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: generate_real_song_midi.py PROJECT.aura")
    project = Path(sys.argv[1]).expanduser().resolve()
    sidecar = Path(f"{project}.midi.json")
    sidecar.write_text(json.dumps(make_song(), indent=2) + "\n", encoding="utf-8")
    total = sum(len(track["notes"]) for track in make_song())
    print(f"wrote {sidecar} ({total} notes, {TOTAL_BARS} bars)")


if __name__ == "__main__":
    main()
