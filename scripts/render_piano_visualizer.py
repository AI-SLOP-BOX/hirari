#!/usr/bin/env python3
"""Shared deterministic falling-note renderer used by Aura UI and CLI."""
import argparse, json, shutil, subprocess, tempfile, wave
from pathlib import Path
from PIL import Image, ImageDraw

def main():
    ap = argparse.ArgumentParser(); ap.add_argument('--notes', required=True); ap.add_argument('--audio', required=True); ap.add_argument('--output', required=True); ap.add_argument('--fps', type=int, default=30); ap.add_argument('--width', type=int, default=1280); ap.add_argument('--height', type=int, default=720); a = ap.parse_args()
    notes = json.loads(Path(a.notes).read_text());
    with wave.open(a.audio, 'rb') as f: duration = f.getnframes() / max(1, f.getframerate())
    frames = Path(tempfile.mkdtemp(prefix='aura-piano-')); lo, hi = 36, 96
    try:
        total = int(duration * a.fps)
        for k in range(total):
            now = k / a.fps; im = Image.new('RGB', (a.width, a.height), (8, 12, 30)); d = ImageDraw.Draw(im); key_w = a.width / (hi - lo + 1); keyboard_top = a.height - 125
            d.rectangle((0, keyboard_top, a.width, a.height), fill=(16, 23, 48))
            for pitch in range(lo, hi + 1):
                x = (pitch - lo) * key_w; d.rectangle((x, a.height - 125, x + key_w - 1, a.height), fill=(235, 238, 245) if pitch % 12 in (0, 2, 4, 5, 7, 9, 11) else (24, 30, 48), outline=(55, 70, 100))
            for note in notes:
                start = float(note.get('start_seconds', note.get('start', 0))); length = float(note.get('duration_seconds', note.get('duration', 0))); pitch = int(note.get('pitch', note.get('midi', 60)))
                # Notes continue behind the physical keyboard. The keyboard
                # layer is drawn afterwards and occludes the lower portion.
                if pitch < lo or pitch > hi or start + length <= now: continue
                x = (pitch - lo) * key_w; y2 = keyboard_top - (start - now) * 72; y1 = y2 - max(10, length * 72)
                if y2 <= 0: continue
                y1 = max(0, y1)
                color = (180, 110, 255) if note.get('role') == 'bass' else (255, 190, 75) if note.get('role') == 'melody' else (70, 220, 255); d.rounded_rectangle((x + 2, y1, x + key_w - 3, y2), radius=5, fill=color, outline=(235, 250, 255))
            # The keyboard is always the foreground layer: notes may approach
            # it, but they must never paint over the keys.
            d.rectangle((0, keyboard_top, a.width, a.height), fill=(10, 14, 26))
            d.rectangle((0, keyboard_top, a.width, keyboard_top + 8), fill=(210, 225, 240))
            for pitch in range(lo, hi + 1):
                active = [n for n in notes if int(n.get('pitch', n.get('midi', 60))) == pitch and float(n.get('start_seconds', n.get('start', 0))) <= now <= float(n.get('start_seconds', n.get('start', 0))) + float(n.get('duration_seconds', n.get('duration', 0)))]
                if active:
                    role = active[-1].get('role'); fill = (180, 110, 255) if role == 'bass' else (255, 190, 75) if role == 'melody' else (70, 220, 255)
                else:
                    fill = (235, 238, 245) if pitch % 12 in (0, 2, 4, 5, 7, 9, 11) else (24, 30, 48)
                x = (pitch - lo) * key_w; d.rectangle((x, keyboard_top + 8, x + key_w - 1, a.height), fill=fill, outline=(55, 70, 100))
            d.line((0, keyboard_top - 1, a.width, keyboard_top - 1), fill=(255, 255, 255), width=2)
            d.text((30, 28), 'AURA / PIANO VISUALIZER', fill=(230, 240, 255)); d.text((32, 62), 'MIDI-synchronized performance', fill=(145, 170, 205)); im.save(frames / f'frame_{k:06d}.jpg', quality=88)
        subprocess.run(['ffmpeg', '-y', '-framerate', str(a.fps), '-i', str(frames / 'frame_%06d.jpg'), '-i', a.audio, '-t', str(duration), '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '256k', '-movflags', '+faststart', a.output], check=True)
    finally: shutil.rmtree(frames, ignore_errors=True)

if __name__ == '__main__': main()
