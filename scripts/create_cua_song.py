#!/usr/bin/env python3
import math
import wave
from pathlib import Path

SR = 16_000
BPM = 112
BEAT = 60.0 / BPM
BARS = 8
DURATION = BARS * 4 * BEAT
OUT = Path("dist/aura_cua_neon_tide.wav")

def midi(n):
    return 440.0 * 2.0 ** ((n - 69) / 12.0)

def env(t, length, attack=0.01, release=0.08):
    if t < attack:
        return t / attack
    if t > length - release:
        return max(0.0, (length - t) / release)
    return 1.0

def add_note(buf, start, length, note, gain, kind="sine"):
    a = max(0, int(start * SR)); b = min(len(buf), int((start + length) * SR))
    f = midi(note)
    for i in range(a, b):
        t = i / SR - start
        e = env(t, length)
        if kind == "pad":
            x = 0.55 * math.sin(2 * math.pi * f * t) + 0.25 * math.sin(2 * math.pi * f * 2 * t) + 0.20 * math.sin(2 * math.pi * f * 0.5 * t)
        elif kind == "bass":
            x = math.sin(2 * math.pi * f * t) + 0.18 * math.sin(2 * math.pi * f * 2 * t)
        else:
            x = math.sin(2 * math.pi * f * t) + 0.20 * math.sin(2 * math.pi * f * 2 * t)
        buf[i] += gain * e * x

def add_drum(buf, start, kind, gain):
    a = int(start * SR); length = 0.22 if kind == "kick" else 0.10
    for j in range(int(length * SR)):
        i = a + j
        if i >= len(buf): break
        t = j / SR
        if kind == "kick":
            x = math.sin(2 * math.pi * (105 - 70 * min(1, t / length)) * t) * math.exp(-18 * t)
        elif kind == "snare":
            x = (2 * ((t * 1800) % 1) - 1) * math.exp(-30 * t) * 0.7 + math.sin(2 * math.pi * 180 * t) * math.exp(-22 * t) * 0.3
        else:
            x = (2 * ((t * 5000) % 1) - 1) * math.exp(-42 * t)
        buf[i] += gain * x

def main():
    n = int(DURATION * SR)
    buf = [0.0] * n
    progression = [(48, 52, 55), (45, 48, 52), (41, 45, 48), (43, 47, 50)]
    for bar in range(BARS):
        chord = progression[bar % 4]
        s = bar * 4 * BEAT
        for note in chord:
            add_note(buf, s, 4 * BEAT * 0.96, note + 12, 0.11, "pad")
        add_note(buf, s, 4 * BEAT * 0.98, chord[0] - 12, 0.18, "bass")
        add_note(buf, s + 2 * BEAT, 2 * BEAT * 0.98, chord[0] - 12, 0.14, "bass")
        for beat in range(4):
            t = s + beat * BEAT
            add_drum(buf, t, "kick", 0.42)
            if beat in (1, 3): add_drum(buf, t, "snare", 0.22)
            add_drum(buf, t + 0.5 * BEAT, "hat", 0.09)
            add_drum(buf, t + 0.75 * BEAT, "hat", 0.06)
    melody = [72, 74, 76, 79, 76, 74, 72, 67, 69, 72, 74, 76, 74, 72, 69, 67]
    for i, note in enumerate(melody):
        add_note(buf, i * 2 * BEAT, 1.55 * BEAT, note, 0.20, "sine")
    peak = max(1.0, max(abs(x) for x in buf))
    OUT.parent.mkdir(exist_ok=True)
    with wave.open(str(OUT), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        frames = bytearray()
        for i, x in enumerate(buf):
            fade = min(1.0, i / (SR * 0.08), (n - i) / (SR * 0.25))
            v = max(-1.0, min(1.0, x / peak * 0.92 * fade))
            frames += int(v * 32767).to_bytes(2, "little", signed=True)
        w.writeframes(frames)
    print(OUT)

if __name__ == "__main__":
    main()
