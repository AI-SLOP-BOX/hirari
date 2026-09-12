#!/usr/bin/env python3
"""Bind an external Chis-A WAV to the exact vocal MIDI track it rendered."""
import argparse
import hashlib
import json
from pathlib import Path


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def vocal_fingerprint(midi_path: Path) -> tuple[str, int]:
    tracks = json.loads(midi_path.read_text(encoding="utf-8"))
    vocal = next(track["notes"] for track in tracks if track["track_id"] == 3)
    canonical = json.dumps(vocal, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return sha256_bytes(canonical), len(vocal)


def provenance(midi_path: Path, wav_path: Path) -> dict:
    midi_hash, note_count = vocal_fingerprint(midi_path)
    return {
        "midi_vocal_sha256": midi_hash,
        "vocal_note_count": note_count,
        "vocal_wav_sha256": sha256_bytes(wav_path.read_bytes()),
        "midi_path": str(midi_path),
        "vocal_wav_path": str(wav_path),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("midi")
    parser.add_argument("wav")
    parser.add_argument("--write", metavar="JSON")
    parser.add_argument("--verify", metavar="JSON")
    args = parser.parse_args()
    current = provenance(Path(args.midi), Path(args.wav))
    if args.verify:
        expected = json.loads(Path(args.verify).read_text(encoding="utf-8"))
        keys = ("midi_vocal_sha256", "vocal_note_count", "vocal_wav_sha256")
        if any(expected.get(key) != current[key] for key in keys):
            raise SystemExit("vocal provenance mismatch: rerender Chis-A for the current MIDI")
    if args.write:
        Path(args.write).write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(current, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
