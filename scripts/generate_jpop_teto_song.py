#!/usr/bin/env python3
"""Render a vocal-first J-pop backing that matches the existing Teto chorus."""

import math
import os
import random
import wave
from array import array
from pathlib import Path

SR = 44100
BPM = 120
BEAT = 60.0 / BPM
BARS = 96
N = int(BARS * 4 * BEAT * SR)
REPO_ROOT = Path(__file__).resolve().parents[1]
OUT = os.environ.get("AURA_JPOP_OUT", str(REPO_ROOT / "dist" / "aura_jpop_teto_instrumental.wav"))
random.seed(2408)


def hz(note):
    return 440.0 * 2.0 ** ((note - 69) / 12.0)


def osc(freq, x, kind="sine"):
    p = (freq * x) % 1.0
    if kind == "saw":
        return 2.0 * p - 1.0
    if kind == "square":
        return 1.0 if p < 0.5 else -1.0
    return math.sin(2.0 * math.pi * p)


def env(x, length, attack=0.01, release=0.1):
    if x < attack:
        return x / attack
    if x > length - release:
        return max(0.0, (length - x) / release)
    return 1.0


def add(buf, start, length, fn, gain=1.0, pan=0.0):
    a = max(0, int(start * SR))
    b = min(N, int((start + length) * SR))
    left = math.sqrt((1.0 - pan) * 0.5)
    right = math.sqrt((1.0 + pan) * 0.5)
    for i in range(a, b):
        v = fn((i - a) / SR, length) * gain
        buf[0][i] += v * left
        buf[1][i] += v * right


def kick(x, _):
    f = 155.0 * math.exp(-x * 22.0) + 46.0
    return math.sin(2.0 * math.pi * f * x) * math.exp(-x * 18.0)


def snare(x, _):
    return (random.uniform(-1.0, 1.0) * 0.8 + math.sin(2.0 * math.pi * 190.0 * x) * 0.2) * math.exp(-x * 28.0)


def hat(x, _):
    return random.uniform(-1.0, 1.0) * math.exp(-x * 95.0)


def piano(note, length):
    f = hz(note)
    def voice(x, _):
        e = math.exp(-x * 3.8) * (1.0 - math.exp(-x * 120.0))
        return (0.72 * osc(f, x) + 0.20 * osc(f * 2.0, x) + 0.08 * osc(f * 3.0, x)) * e
    return voice


def bass(note, length):
    f = hz(note)
    def voice(x, _):
        e = env(x, length, 0.006, min(0.12, length * 0.3))
        return (0.78 * osc(f, x, "saw") + 0.22 * osc(f * 0.5, x)) * e
    return voice


def pad(notes, length):
    def voice(x, _):
        e = env(x, length, 0.28, 0.5)
        return sum(osc(hz(n), x, "saw") * (0.65 + 0.35 * math.sin(2 * math.pi * 0.4 * x)) for n in notes) / len(notes) * e
    return voice


def guitar(note, length):
    f = hz(note)
    def voice(x, _):
        e = math.exp(-x * 7.0) * (1.0 - math.exp(-x * 180.0))
        return (0.55 * osc(f, x, "square") + 0.45 * osc(f * 2.0, x, "saw")) * e
    return voice


def render():
    drums = [[0.0] * N, [0.0] * N]
    bass_buf = [[0.0] * N, [0.0] * N]
    music = [[0.0] * N, [0.0] * N]
    hook = [[0.0] * N, [0.0] * N]
    # Am-F-C-G: same harmonic bed as the Teto chorus, with a brighter C/E lift.
    chords = [(57, 60, 64, 69), (53, 57, 60, 65), (48, 52, 55, 60), (55, 59, 62, 67)]
    roots = [33, 29, 36, 31]

    for bar in range(BARS):
        t0 = bar * 4 * BEAT
        in_section = bar % 16
        section = "intro" if bar < 8 else "verse" if bar < 16 or 40 <= bar < 48 else "chorus" if 16 <= bar < 32 or 48 <= bar < 64 or bar >= 80 else "bridge"
        chord = chords[bar % 4]
        root = roots[bar % 4]

        # J-pop drum arc: half-time verse, full chorus, restrained bridge.
        if section != "intro" or bar >= 4:
            for beat in (0.0, 2.0) if section in ("verse", "bridge") else (0.0, 1.5, 2.0, 3.25):
                add(drums, t0 + beat * BEAT, 0.22, kick, 0.52 if section == "verse" else 0.64)
            for beat in (1.0, 3.0):
                add(drums, t0 + beat * BEAT, 0.18, snare, 0.30 if section == "verse" else 0.42, 0.05)
            step = 1.0 if section in ("verse", "bridge") else 0.5
            beat = 0.5
            while beat < 4.0:
                add(drums, t0 + beat * BEAT, 0.055, hat, 0.08 if section == "verse" else 0.12, -0.2 if int(beat * 2) % 2 else 0.2)
                beat += step

        # Piano plays the emotional bed; chorus opens the voicing upward.
        for beat, index in enumerate((0, 1, 2, 1)):
            if section != "intro" or bar >= 2:
                add(music, t0 + beat * BEAT, 0.78 * BEAT, piano(chord[index] + 12, 0.78 * BEAT), 0.13, -0.16 if beat % 2 else 0.16)
        if section != "bridge":
            add(music, t0, 3.8 * BEAT, pad(chord, 3.8 * BEAT), 0.07 if section == "verse" else 0.11, 0.0)

        # Bass leaves space for the vocal in the verse and becomes syncopated in chorus.
        if section != "intro":
            pattern = (0.0, 2.0) if section in ("verse", "bridge") else (0.0, 1.5, 2.0, 3.0)
            for beat in pattern:
                n = root + (12 if beat == 2.0 and section == "chorus" else 0)
                add(bass_buf, t0 + beat * BEAT, 0.62 * BEAT, bass(n, 0.62 * BEAT), 0.20 if section == "verse" else 0.26, -0.04)

        # A small answer phrase, never occupying the vocal melody's register.
        if section == "chorus":
            answer = (chord[2] + 12, chord[1] + 12, chord[0] + 12, chord[1] + 12)
            for step, note in enumerate(answer):
                add(hook, t0 + (step * 2 + 1) * 0.5 * BEAT, 0.28 * BEAT, guitar(note, 0.28 * BEAT), 0.07, 0.22 if step % 2 else -0.22)

    mix = [[0.0] * N, [0.0] * N]
    for i in range(N):
        for ch in (0, 1):
            mix[ch][i] = drums[ch][i] + bass_buf[ch][i] + music[ch][i] + hook[ch][i]
    peak = max(1e-9, max(abs(v) for channel in mix for v in channel))
    scale = 0.78 / peak
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with wave.open(OUT, "wb") as wf:
        wf.setnchannels(2); wf.setsampwidth(2); wf.setframerate(SR)
        pcm = array("h")
        for i in range(N):
            for ch in (0, 1):
                pcm.append(max(-32767, min(32767, int(math.tanh(mix[ch][i] * scale) * 0.82 * 32767))))
        wf.writeframes(pcm.tobytes())
    print(OUT)


if __name__ == "__main__":
    render()
