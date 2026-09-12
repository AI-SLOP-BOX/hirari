#!/usr/bin/env python3
"""Build a short accompaniment-only MIDI tail from an Aura note JSON file."""

import json
import struct
import sys
from pathlib import Path

PPQ = 480
BPM = 110
DEFAULT_TAIL_START_BEAT = 352.0  # bar 88 at 110 BPM


def vlq(value: int) -> bytes:
    encoded = [value & 0x7F]
    value >>= 7
    while value:
        encoded.append((value & 0x7F) | 0x80)
        value >>= 7
    return bytes(reversed(encoded))


def make_track(events: list[tuple[int, bytes]]) -> bytes:
    body = bytearray()
    cursor = 0
    for tick, payload in sorted(events, key=lambda item: (item[0], item[1][0])):
        body.extend(vlq(max(0, tick - cursor)))
        body.extend(payload)
        cursor = tick
    body.extend(b"\x00\xff\x2f\x00")
    return b"MTrk" + struct.pack(">I", len(body)) + body


def main() -> int:
    if len(sys.argv) not in (3, 4):
        raise SystemExit("usage: build_tail_midi.py INPUT_JSON OUTPUT_MID [TAIL_START_BEAT]")
    tail_start_beat = float(sys.argv[3]) if len(sys.argv) == 4 else DEFAULT_TAIL_START_BEAT
    tracks = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    midi_tracks = [
        make_track([
            (0, b"\xff\x51\x03" + struct.pack(">I", 60_000_000 // BPM)[1:]),
            (0, b"\xff\x58\x04\x04\x02\x18\x08"),
        ])
    ]
    kept = 0
    for source in sorted(tracks, key=lambda item: int(item.get("track_id", 0))):
        track_id = int(source.get("track_id", 0))
        if track_id == 3:  # Chis-A vocal is not part of the Surge tail render.
            continue
        channel = track_id % 16
        events = []
        for note in source.get("notes", []):
            start_beat = float(note["start_beat"])
            end_beat = start_beat + float(note["length_beats"])
            if end_beat <= tail_start_beat:
                continue
            start = max(0, round((start_beat - tail_start_beat) * PPQ))
            length = max(1, round((end_beat - max(start_beat, tail_start_beat)) * PPQ))
            pitch = max(0, min(127, int(note["pitch"])))
            velocity = max(1, min(127, int(note.get("velocity", 96))))
            events.append((start, bytes((0x90 | channel, pitch, velocity))))
            events.append((start + length, bytes((0x80 | channel, pitch, 0))))
            kept += 1
        midi_tracks.append(make_track(events))
    output = b"MThd" + struct.pack(">IHHH", 6, 1, len(midi_tracks), PPQ) + b"".join(midi_tracks)
    Path(sys.argv[2]).write_bytes(output)
    print(json.dumps({"ok": True, "tracks": len(midi_tracks), "notes": kept,
                      "tail_start_beat": tail_start_beat, "output": sys.argv[2]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
