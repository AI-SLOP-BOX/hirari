#!/usr/bin/env python3
"""Build a reversible vocal-density candidate from an auditable source render.

This never replaces the canonical singer render.  It creates a low-level octave
support layer with Rubber Band and mixes it back into the source at the same
duration/sample format, so the candidate can be auditioned and rejected safely.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


def run(command: list[str]) -> None:
    subprocess.run(command, check=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--provenance", type=Path)
    args = parser.parse_args()
    if not shutil.which("rubberband") or not shutil.which("ffmpeg"):
        raise SystemExit("rubberband and ffmpeg are required")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="aura-vocal-harmony-") as temp:
        harmony = Path(temp) / "octave.wav"
        run(["rubberband", "-p", "12", "-t", "1.0", str(args.source), str(harmony)])
        run([
            "ffmpeg", "-y", "-i", str(args.source), "-i", str(harmony),
            "-filter_complex",
            "[0:a]volume=0.92[lead];"
            "[1:a]volume=0.16,highpass=f=180,lowpass=f=7000,"
            "adelay=18|18,stereotools=mpan=0.72[support];"
            "[lead][support]amix=inputs=2:duration=first:normalize=0,"
            "alimiter=limit=0.95",
            "-ar", "48000", "-ac", "2", "-c:a", "pcm_s16le", str(args.output),
        ])
    if args.provenance:
        args.provenance.write_text(json.dumps({
            "kind": "reversible_vocal_density_candidate",
            "source": str(args.source),
            "output": str(args.output),
            "method": "rubberband_octave_support_plus_stereo_delay",
            "support_gain": 0.16,
            "delay_ms": 18,
            "canonical_replaced": False,
        }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
