#!/usr/bin/env python3
"""Create an Aura project whose audible assets are native audio regions.

The MIDI sidecar remains in the project for editability, while the rendered
vocal and instrument are loaded as separate regions so Aura's own offline
engine performs the final summing/bounce.
"""

import json
import subprocess
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit("usage: build_aura_native_session.py SOURCE_AURA VOCAL_WAV INSTRUMENT_WAV OUTPUT_AURA")
    source, vocal, instrument, output = map(lambda value: Path(value).expanduser().resolve(), sys.argv[1:])
    data = json.loads(source.read_text(encoding="utf-8"))
    output.parent.mkdir(parents=True, exist_ok=True)
    assets = output.parent / "native_assets"
    assets.mkdir(exist_ok=True)

    def copy_44100(src: Path, name: str) -> Path:
        dst = assets / name
        subprocess.run(
            ["ffmpeg", "-y", "-i", str(src), "-ar", "44100", "-ac", "2", "-c:a", "pcm_s16le", str(dst)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return dst

    vocal_path = copy_44100(vocal, "vocal.wav")
    instrument_path = copy_44100(instrument, "instrument.wav")
    # Store portable project-relative references. Aura resolves these against
    # the project directory during hydration, so the complete session can be
    # moved or archived without silently losing its audio assets.
    vocal_ref = "native_assets/vocal.wav"
    instrument_ref = "native_assets/instrument.wav"
    import wave

    def frames(path: Path) -> int:
        with wave.open(str(path), "rb") as handle:
            return handle.getnframes()

    vocal_frames = frames(vocal_path)
    instrument_frames = frames(instrument_path)
    length = max(vocal_frames, instrument_frames)
    for track in data["tracks"]:
        if track["id"] == 1:
            track.update({"name": "Aura Instrument Bus", "track_type": "Audio", "volume": 0.78, "pan": 0.0})
        elif track["id"] == 4:
            track.update({"name": "Aura Chis-A Vocal", "track_type": "Audio", "volume": 1.0, "pan": 0.0})
    data["master_gain"] = 0.82
    data["regions"] = [
        {"id": 1001, "track_id": 1, "name": "Aura instrument render", "path": instrument_ref, "start": 0,
         "length": instrument_frames, "source_offset": 0, "base_source_offset": 0, "base_length": instrument_frames,
         "muted": False, "clip_gain": 1.0, "fade_in_samples": 2205, "fade_out_samples": 176400,
         "warp_ratio": 1.0, "pitch_semitones": 0.0, "reverse": False, "loop_count": 1},
        {"id": 1002, "track_id": 4, "name": "Aura Chis-A render", "path": vocal_ref, "start": 0,
         "length": vocal_frames, "source_offset": 0, "base_source_offset": 0, "base_length": vocal_frames,
         "muted": False, "clip_gain": 1.0, "fade_in_samples": 2205, "fade_out_samples": 176400,
         "warp_ratio": 1.0, "pitch_semitones": 0.0, "reverse": False, "loop_count": 1},
    ]
    data["metadata"]["native_audio_session"] = True
    data["metadata"]["native_audio_session_length_samples"] = length
    output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"ok": True, "project": str(output), "regions": len(data["regions"]), "length_samples": length}))


if __name__ == "__main__":
    main()
