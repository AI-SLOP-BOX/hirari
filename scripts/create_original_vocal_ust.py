#!/usr/bin/env python3
"""Write a short original Japanese UTAU score for Aura's synth demo."""
from pathlib import Path
import os
import sys

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/aura_original_song.ust")
notes = [62, 64, 67, 69, 67, 64, 62, 60, 62, 65, 69, 72, 69, 67, 65, 62]
lyrics = ["よ", "る", "を", "さ", "い", "て", "ま", "え", "へ", "す", "す", "め", "ひ", "か", "り", "へ"]
lines = ["[#SETTING]", f"Tempo={os.environ.get('AURA_VOCAL_TEMPO', '150')}", "ProjectName=Aura Original Song", "VoiceDir=", "OutFile=aura_neon_original.wav", "Mode2=True", "[#VERSION]", "UST Version1.2"]
for i, (pitch, lyric) in enumerate(zip(notes, lyrics)):
    lines += [f"[#{i:04d}]", "Length=480", f"Lyric={lyric}", f"NoteNum={pitch}", "PreUtterance=0"]
OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
print(OUT)
