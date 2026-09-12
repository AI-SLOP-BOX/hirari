#!/usr/bin/env python3
"""Render an original, vocal-free Canon progression and Synthesia-style video."""
import math, os, wave, json, subprocess
from array import array
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "dist"
SR, BPM, BARS, FPS = 44100, 96, 72, 10
BEAT = 60.0 / BPM
DURATION = BARS * 4 * BEAT

def hz(n): return 440.0 * 2.0 ** ((n - 69) / 12.0)
def osc(f, t):
    p = (f * t) % 1.0
    return 2.0 * p - 1.0

def piano_voice(f, t, velocity, bass=False):
    attack = 1.0 - math.exp(-t * (150.0 if bass else 190.0))
    decay = math.exp(-t * (2.2 if bass else 4.6))
    stretch = 1.0 + 0.0008 * (f / 440.0)
    partials = (1.0, 0.46, 0.22, 0.105, 0.045, 0.018)
    tone = sum(weight * math.sin(2.0 * math.pi * f * (i + 1) * stretch * t) for i, weight in enumerate(partials))
    hammer = 0.09 * math.exp(-t * 70.0) * math.sin(2.0 * math.pi * f * 2.7 * t)
    return (tone + hammer) * attack * decay * (0.55 + velocity * 0.45)

def note_events():
    # Pachelbel's public-domain harmonic cycle, with a new melody and voicing.
    progression = [(38, [62, 66, 69, 74]), (33, [61, 64, 69, 73]), (35, [62, 66, 71, 74]), (30, [61, 66, 69, 73]),
                   (31, [59, 62, 67, 71]), (38, [62, 66, 69, 74]), (31, [59, 62, 67, 71]), (33, [61, 64, 69, 73])]
    events = []
    canonical_melody = [
        [78, 76, 74, 73, 71, 69, 71, 73],
        [74, 73, 71, 69, 67, 66, 67, 69],
        [71, 69, 67, 66, 64, 62, 64, 66],
        [67, 66, 64, 62, 61, 59, 61, 62],
        [66, 64, 62, 61, 62, 64, 66, 67],
        [69, 67, 66, 64, 66, 67, 69, 71],
        [74, 73, 71, 69, 71, 73, 74, 76],
        [78, 76, 74, 73, 74, 76, 78, 79],
    ]
    for bar in range(BARS):
        t = bar * 4 * BEAT
        root, chord = progression[bar % len(progression)]
        for step in range(8):
            events.append((t + step * .5 * BEAT, .42 * BEAT, chord[step % 4], 0.20, "arp"))
        events.append((t, 3.8 * BEAT, root, 0.22, "bass"))
        events.append((t + 2 * BEAT, 1.8 * BEAT, root + 12, 0.10, "bass"))
        melody = canonical_melody[bar % len(canonical_melody)]
        for i, pitch in enumerate(melody):
            events.append((t + i * .5 * BEAT, .44 * BEAT, pitch, 0.16, "melody"))
    return events

def render_audio(events, path):
    n = int(DURATION * SR); left = [0.0] * n; right = [0.0] * n
    for start, length, pitch, gain, role in events:
        a, b = int(start * SR), min(n, int((start + length) * SR)); f = hz(pitch)
        for i in range(max(0, a), b):
            t = (i - a) / SR; attack = min(1.0, t / .012); release = min(1.0, max(0.0, (length - t) / .09)); e = attack * release
            v = piano_voice(f, t, min(1.0, gain * 4.0), bass=role == "bass")
            pan = -0.18 if role == "arp" and pitch % 2 else 0.18 if role == "arp" else 0.0
            left[i] += v * gain * e * math.sqrt((1 - pan) * .5); right[i] += v * gain * e * math.sqrt((1 + pan) * .5)
    peak = max(1e-6, max(max(abs(x) for x in left), max(abs(x) for x in right))); scale = .78 / peak
    path.parent.mkdir(exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(SR)
        pcm = array("h")
        for l, r in zip(left, right): pcm.extend((int(max(-1, min(1, math.tanh(l * scale))) * 32767), int(max(-1, min(1, math.tanh(r * scale))) * 32767)))
        w.writeframes(pcm.tobytes())

def render_video(events, audio, output):
    frames = OUT / "canon_synthesia_frames"; frames.mkdir(exist_ok=True)
    lo, hi = 48, 86
    for frame in range(int(DURATION * FPS)):
        now = frame / FPS; im = Image.new("RGB", (1280, 720), (8, 12, 30)); d = ImageDraw.Draw(im)
        d.rectangle((0, 570, 1280, 720), fill=(16, 23, 48)); key_w = 1280 / (hi - lo + 1)
        for p in range(lo, hi + 1):
            x = (p - lo) * key_w; d.rectangle((x, 570, x + key_w - 1, 719), outline=(55, 70, 100), fill=(235, 238, 245) if p % 12 in (0, 2, 4, 5, 7, 9, 11) else (24, 30, 48))
        for start, length, pitch, gain, role in events:
            if pitch < lo or pitch > hi or start > now + 5.0 or start + length < now - .1: continue
            x = (pitch - lo) * key_w; y2 = 570 - (start - now) * 70; y1 = y2 - max(10, length * 70)
            color = (70, 220, 255) if role == "arp" else (255, 190, 75) if role == "melody" else (180, 110, 255)
            d.rounded_rectangle((x + 2, y1, x + key_w - 3, y2), radius=5, fill=color, outline=(235, 250, 255))
        d.text((36, 34), "CANON / AURA", fill=(230, 240, 255)); d.text((38, 76), "original canon arrangement · tuned OpenUtau vocal", fill=(145, 170, 205))
        im.save(frames / f"frame_{frame:05d}.jpg", quality=86, optimize=True)
    subprocess.run(["ffmpeg", "-y", "-framerate", str(FPS), "-i", str(frames / "frame_%05d.jpg"), "-i", str(audio), "-t", str(DURATION), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart", str(output)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

if __name__ == "__main__":
    events = note_events(); audio = OUT / "canon_synth_master.wav"; video = OUT / "canon_synthesia.mp4"
    (OUT / "canon_synth_notes.json").write_text(json.dumps([{"start": s, "duration": d, "midi": p, "role": r} for s, d, p, _, r in events], indent=2))
    render_audio(events, audio); render_video(events, audio, video); print(audio); print(video)
