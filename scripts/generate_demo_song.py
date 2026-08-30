import math
import os
import random
import wave
from array import array

SR = 44100
BPM = 100
BEAT = 60.0 / BPM
BARS = 32
DURATION = BARS * 4 * BEAT
N = int(DURATION * SR)
OUT = os.environ.get("AURA_SONG_OUT", "/Users/REDACTED/Desktop/logicpro_oss/dist/aura_demo_song.wav")
STEMS = os.path.splitext(OUT)[0] + "_stems"
random.seed(17)

def midi(n):
    return 440.0 * 2.0 ** ((n - 69) / 12.0)

def sec(t):
    return max(0, min(N - 1, int(t * SR)))

def add(buf, start, dur, fn, gain=1.0, pan=0.0):
    a, b = sec(start), min(N, sec(start + dur))
    left = math.sqrt((1.0 - pan) * 0.5)
    right = math.sqrt((1.0 + pan) * 0.5)
    for i in range(a, b):
        x = (i - a) / SR
        v = fn(x, dur) * gain
        buf[0][i] += v * left
        buf[1][i] += v * right

def env(x, dur, attack=0.008, release=0.12):
    if x < attack:
        return x / attack
    if x > dur - release:
        return max(0.0, (dur - x) / release)
    return 1.0

def osc(freq, kind, x):
    phase = (freq * x) % 1.0
    if kind == "saw":
        return 2.0 * phase - 1.0
    if kind == "square":
        return 1.0 if phase < 0.5 else -1.0
    return math.sin(2.0 * math.pi * phase)

def kick(x, dur):
    f = 145.0 * math.exp(-x * 18.0) + 43.0
    return math.sin(2 * math.pi * f * x) * math.exp(-x * 18.0)

def snare(x, dur):
    noise = random.uniform(-1.0, 1.0)
    tone = math.sin(2 * math.pi * 190.0 * x)
    return (noise * 0.78 + tone * 0.22) * math.exp(-x * 24.0)

def hat(x, dur):
    return random.uniform(-1.0, 1.0) * math.exp(-x * 80.0)

def bass_note(note, length, accent=1.0):
    f = midi(note)
    def voice(x, dur):
        e = env(x, dur, 0.006, min(0.16, dur * 0.4))
        wobble = 0.5 + 0.5 * math.sin(2 * math.pi * 2.1 * x)
        return (0.78 * osc(f, "saw", x) + 0.22 * math.sin(2 * math.pi * f * 0.5 * x)) * e * (0.82 + 0.18 * wobble)
    return voice

def pad_chord(notes, length):
    def voice(x, dur):
        e = env(x, dur, 0.35, 0.6)
        return sum(osc(midi(n) * (1.0 + 0.002 * j), "saw", x) for j, n in enumerate(notes)) / len(notes) * e
    return voice

def pluck(note, length):
    f = midi(note)
    def voice(x, dur):
        e = math.exp(-x * 5.5) * (1.0 - math.exp(-x * 120.0))
        return (0.62 * osc(f, "square", x) + 0.38 * osc(f * 2.0, "saw", x)) * e
    return voice

def lead(note, length):
    f = midi(note)
    def voice(x, dur):
        e = env(x, dur, 0.025, min(0.18, dur * 0.3))
        vibrato = 1.0 + 0.004 * math.sin(2 * math.pi * 5.2 * x)
        return (0.66 * osc(f * vibrato, "saw", x) + 0.34 * math.sin(2 * math.pi * f * vibrato * x)) * e
    return voice

def render():
    drums = [[0.0] * N, [0.0] * N]
    bass = [[0.0] * N, [0.0] * N]
    music = [[0.0] * N, [0.0] * N]
    lead_buf = [[0.0] * N, [0.0] * N]
    chords = [[50, 53, 57], [46, 50, 53], [43, 46, 50], [48, 52, 55]]
    roots = [38, 34, 31, 36]
    for bar in range(BARS):
        t0 = bar * 4 * BEAT
        section = "intro" if bar < 4 else "verse" if bar < 12 else "chorus" if bar < 20 else "bridge" if bar < 24 else "chorus"
        chord_i = bar % 4
        if section != "intro":
            add(music, t0, 4 * BEAT, pad_chord(chords[chord_i], 4 * BEAT), 0.17, -0.08)
        for beat in range(4):
            bt = t0 + beat * BEAT
            if section != "intro" or beat in (0, 2):
                add(drums, bt, 0.22, kick, 0.72, 0.0)
            if beat in (1, 3) and section != "intro":
                add(drums, bt, 0.18, snare, 0.42, 0.04)
            if section in ("chorus", "bridge"):
                add(drums, bt + 0.5 * BEAT, 0.06, hat, 0.14, 0.25 if beat % 2 else -0.25)
            elif section == "verse":
                add(drums, bt + 0.5 * BEAT, 0.05, hat, 0.09, -0.2)
        if section != "intro":
            for step in range(8):
                bt = t0 + step * 0.5 * BEAT
                note = roots[chord_i] + (12 if step in (3, 7) else 0)
                add(bass, bt, 0.34 * BEAT, bass_note(note, 0.34 * BEAT), 0.33, -0.02)
        if section in ("verse", "chorus"):
            pattern = [0, 2, 1, 2, 0, 2, 3, 2]
            for step, scale in enumerate(pattern):
                bt = t0 + step * 0.5 * BEAT
                add(music, bt, 0.28 * BEAT, pluck(chords[chord_i][0] + scale * 2 + 12, 0.28 * BEAT), 0.13, 0.18 if step % 2 else -0.18)
        if section == "chorus":
            melody = [62, 65, 69, 67, 65, 62, 60, 62]
            for step, note in enumerate(melody):
                bt = t0 + step * 0.5 * BEAT
                add(lead_buf, bt, 0.42 * BEAT, lead(note, 0.42 * BEAT), 0.20, 0.1 * math.sin(step))
    # Gentle bus ducking keyed by kick, plus soft saturation/limiting.
    mix = [[0.0] * N, [0.0] * N]
    for i in range(N):
        t = i / SR
        duck = 1.0
        for k in range(max(0, int(t / (4 * BEAT)) - 1), int(t / (4 * BEAT)) + 2):
            for beat in range(4):
                kt = k * 4 * BEAT + beat * BEAT
                d = t - kt
                if 0 <= d < 0.24:
                    duck = min(duck, 0.72 + 0.28 * min(1.0, d / 0.24))
        for ch in (0, 1):
            mix[ch][i] = drums[ch][i] + bass[ch][i] * duck + music[ch][i] * duck + lead_buf[ch][i] * duck
    peak = max(1e-9, max(abs(v) for ch in mix for v in ch))
    scale = 0.86 / peak
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with wave.open(OUT, "wb") as wf:
        wf.setnchannels(2); wf.setsampwidth(2); wf.setframerate(SR)
        pcm = array("h")
        for i in range(N):
            for ch in (0, 1):
                v = math.tanh(mix[ch][i] * scale * 1.25) * 0.78
                pcm.append(max(-32767, min(32767, int(v * 32767))))
        wf.writeframes(pcm.tobytes())
    for name, stem in (("drums", drums), ("bass", bass), ("music", music), ("lead", lead_buf)):
        path = os.path.join(STEMS, name + ".wav")
        os.makedirs(STEMS, exist_ok=True)
        with wave.open(path, "wb") as wf:
            wf.setnchannels(2); wf.setsampwidth(2); wf.setframerate(SR)
            pcm = array("h")
            for i in range(N):
                for ch in (0, 1):
                    v = math.tanh(stem[ch][i] * 1.1) * 0.62
                    pcm.append(max(-32767, min(32767, int(v * 32767))))
            wf.writeframes(pcm.tobytes())
    print(OUT)
    print(STEMS)

if __name__ == "__main__":
    render()
