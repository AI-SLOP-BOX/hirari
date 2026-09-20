import os
import wave
from array import array
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DIST = REPO_ROOT / "dist"
INSTRUMENTAL = os.environ.get("AURA_TETO_INSTRUMENTAL", str(DIST / "aura_demo_song.wav"))
VOCAL = os.environ.get("AURA_TETO_VOCAL", str(Path.home() / "Documents" / "aura_teto_vocal.wav"))
OUT = os.environ.get("AURA_TETO_MIX", str(DIST / "aura_demo_song_teto.wav"))
SR = 44100

def read_mono(path):
    with wave.open(path, "rb") as wf:
        assert wf.getframerate() == SR
        channels = wf.getnchannels()
        frames = wf.getnframes()
        raw = array("h", wf.readframes(frames))
    if channels == 1:
        return [x / 32768.0 for x in raw]
    return [sum(raw[i + ch] for ch in range(channels)) / (32768.0 * channels) for i in range(0, len(raw), channels)]

def main():
    with wave.open(INSTRUMENTAL, "rb") as wf:
        assert wf.getframerate() == SR and wf.getnchannels() == 2
        frames = wf.getnframes()
        music = array("h", wf.readframes(frames))
    vocal = read_mono(VOCAL)
    # The installed Teto render contains three chorus phrases. Use the first
    # phrase and place it over the second half of the demo chorus.
    src_start = 40 * SR
    src_end = min(len(vocal), 70 * SR)
    phrase = vocal[src_start:src_end]
    dst_start = 24 * SR
    for j, sample in enumerate(phrase):
        i = dst_start + j
        if i >= frames:
            break
        # Keep the synth bed audible while making the singer clearly present.
        fade = min(1.0, j / (SR * 0.35), (len(phrase) - j) / (SR * 0.45))
        v = sample * 0.74 * max(0.0, fade)
        for ch in (0, 1):
            mixed = music[2 * i + ch] / 32768.0 + v
            music[2 * i + ch] = max(-32767, min(32767, int(max(-0.98, min(0.98, mixed)) * 32767)))
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with wave.open(OUT, "wb") as wf:
        wf.setnchannels(2); wf.setsampwidth(2); wf.setframerate(SR)
        wf.writeframes(music.tobytes())
    print(OUT)

if __name__ == "__main__":
    main()
