#!/usr/bin/env python3
"""Convert Aura vocal note JSON into a classic OpenUtau UST candidate."""

import json
import sys
from pathlib import Path

PPQ = 480
VOICE_DIR = "/Users/REDACTED/Library/Application Support/OpenUtau/Singers/KasaneTetoOfficial"


def main() -> int:
    if len(sys.argv) not in (3, 4):
        raise SystemExit("usage: json_to_ust_candidate.py INPUT_JSON OUTPUT_UST [VOICE_DIR]")
    tracks = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    vocal = next(track["notes"] for track in tracks if int(track["track_id"]) == 3)
    notes = sorted(vocal, key=lambda note: float(note["start_beat"]))
    voice_dir = Path(sys.argv[3]).expanduser() if len(sys.argv) == 4 else Path(VOICE_DIR)
    lines = [
        "[#SETTING]", "Tempo=110", "ProjectName=Aura Candidate Teto",
        f"VoiceDir={voice_dir}", "OutFile=aura_candidate_teto.wav", "Mode2=True",
        "[#VERSION]", "UST Version1.2",
    ]
    for index, note in enumerate(notes):
        length = max(120, round(float(note["length_beats"]) * PPQ))
        lyric = str(note.get("lyric", "あ")) or "あ"
        pitch = max(36, min(96, int(note["pitch"])))
        velocity = max(45, min(110, int(note.get("velocity", 96))))
        lines.extend([
            f"[#{index:04d}]", f"Length={length}", f"Lyric={lyric}",
            f"NoteNum={pitch}", f"Intensity={velocity}", "Modulation=0",
            "PreUtterance=42", "VoiceOverlap=0", "Piches=0,0,2,0,0",
            "VBR=0,190,0,0,0,0",
        ])
    lines.append("[#TRACKEND]")
    Path(sys.argv[2]).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"ok": True, "notes": len(notes), "output": sys.argv[2]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
