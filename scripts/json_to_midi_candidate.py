#!/usr/bin/env python3
"""Convert a candidate Aura note JSON file to a compact Type-1 MIDI file."""

import json
import struct
import sys
from pathlib import Path

PPQ = 480
BPM = 110


def vlq(value: int) -> bytes:
    encoded = [value & 0x7F]
    value >>= 7
    while value:
        encoded.append((value & 0x7F) | 0x80)
        value >>= 7
    return bytes(reversed(encoded))


def track(events: list[tuple[int, bytes]]) -> bytes:
    body = bytearray()
    cursor = 0
    for tick, payload in sorted(events, key=lambda item: (item[0], item[1][0])):
        body.extend(vlq(max(0, tick - cursor)))
        body.extend(payload)
        cursor = tick
    body.extend(b"\x00\xff\x2f\x00")
    return b"MTrk" + struct.pack(">I", len(body)) + body


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: json_to_midi_candidate.py INPUT_JSON OUTPUT_MID")
    tracks = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    midi_tracks = [
        track([
            (0, b"\xff\x51\x03" + struct.pack(">I", 60_000_000 // BPM)[1:]),
            (0, b"\xff\x58\x04\x04\x02\x18\x08"),
        ])
    ]
    for source in sorted(tracks, key=lambda item: int(item.get("track_id", 0))):
        channel = int(source.get("track_id", 0)) % 16
        events = []
        for note in source.get("notes", []):
            start = round(float(note["start_beat"]) * PPQ)
            length = max(1, round(float(note["length_beats"]) * PPQ))
            pitch = max(0, min(127, int(note["pitch"])))
            velocity = max(1, min(127, int(note.get("velocity", 96))))
            events.append((start, bytes((0x90 | channel, pitch, velocity))))
            events.append((start + length, bytes((0x80 | channel, pitch, 0))))
            lyric = str(note.get("lyric", ""))
            if lyric:
                encoded = lyric.encode("utf-8")
                events.append((start, b"\xff\x05" + vlq(len(encoded)) + encoded))
        midi_tracks.append(track(events))
    output = b"MThd" + struct.pack(">IHHH", 6, 1, len(midi_tracks), PPQ) + b"".join(midi_tracks)
    Path(sys.argv[2]).write_bytes(output)
    print(json.dumps({"ok": True, "tracks": len(midi_tracks), "output": sys.argv[2]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
