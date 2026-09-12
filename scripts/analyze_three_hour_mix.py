#!/usr/bin/env python3
"""Measure the three-hour release inputs and reject obvious mix regressions."""
import json
import math
import pathlib
import struct
import subprocess
import sys
import re


def decode_mono(path: pathlib.Path, sample_rate: int = 44100):
    proc = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(path), "-f", "s32le", "-ac", "1", "-ar", str(sample_rate), "pipe:1"],
        check=True,
        stdout=subprocess.PIPE,
    )
    raw = proc.stdout
    values = [x[0] / 2147483648.0 for x in struct.iter_unpack("<i", raw)]
    if not values:
        raise ValueError(f"empty audio: {path}")
    peak = max(abs(x) for x in values)
    rms = math.sqrt(sum(x * x for x in values) / len(values))
    return values, peak, rms


def db(value: float) -> float:
    return 20.0 * math.log10(max(value, 1e-12))


def loudness(path: pathlib.Path) -> float | None:
    proc = subprocess.run(
        ["ffmpeg", "-v", "info", "-i", str(path), "-filter_complex", "ebur128=framelog=verbose", "-f", "null", "-"],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    text = proc.stderr
    integrated = re.findall(r"I:\s+(-?\d+(?:\.\d+)?)\s+LUFS", text)
    return float(integrated[-1]) if integrated else None


def window_rms(values: list[float], start: int, end: int) -> float:
    chunk = values[start:end]
    return math.sqrt(sum(value * value for value in chunk) / max(len(chunk), 1))


def main() -> int:
    if len(sys.argv) != 5:
        print("usage: analyze_three_hour_mix.py VOCAL INSTRUMENT MIX OUT_JSON", file=sys.stderr)
        return 2
    vocal_path, inst_path, mix_path, out_path = map(pathlib.Path, sys.argv[1:])
    vocal, vocal_peak, vocal_rms = decode_mono(vocal_path)
    inst, inst_peak, inst_rms = decode_mono(inst_path)
    mix, mix_peak, mix_rms = decode_mono(mix_path)
    lufs = loudness(mix_path)
    # These are the fixed pre-mix gains in run_three_hour_release.sh. Keeping
    # them here makes the report describe the audible balance, not just the
    # unprocessed source files.
    vocal_mix_gain = 2.5
    instrument_mix_gain = 0.74
    section_samples = int(8 * 4 * 60 / 110 * 44100)
    section_db = []
    section_balance_db = []
    for start in range(0, len(mix), section_samples):
        if start >= len(mix):
            break
        end = min(start + section_samples, len(mix))
        if end - start < section_samples // 2:
            break
        # The final four seconds are an intentional release fade; exclude
        # that silence from the arrangement-contrast measurement.
        if end == len(mix):
            end = max(start + 1, end - int(4 * 44100))
        section_db.append(db(window_rms(mix, start, end)))
        vocal_window = window_rms(vocal, start, min(end, len(vocal)))
        instrument_window = window_rms(inst, start, min(end, len(inst)))
        if vocal_window > 0.01 and instrument_window > 0.001:
            section_balance_db.append(db((vocal_window * vocal_mix_gain) / (instrument_window * instrument_mix_gain)))
    section_range = max(section_db) - min(section_db)
    tail_start = max(0, len(inst) - int(24 * 44100))
    tail_instrument_rms = window_rms(inst, tail_start, len(inst))
    n = min(len(vocal), len(inst), len(mix))
    if n < 44100:
        raise ValueError("release is shorter than one second")
    numerator = sum(vocal[i] * inst[i] for i in range(n))
    denom = math.sqrt(sum(vocal[i] ** 2 for i in range(n)) * sum(inst[i] ** 2 for i in range(n)))
    correlation = numerator / denom if denom else 0.0
    estimated_balance = db((vocal_rms * vocal_mix_gain) / max(inst_rms * instrument_mix_gain, 1e-12))
    active_section_balance_min = min(section_balance_db) if section_balance_db else -99.0
    mix_duration = len(mix) / 44100.0
    vocal_duration = len(vocal) / 44100.0
    duration_error = abs(mix_duration - vocal_duration)
    result = {
        "ok": mix_peak <= 0.99 and mix_duration >= 180.0 and duration_error <= 0.5 and estimated_balance >= -3.0 and active_section_balance_min >= -8.0 and section_range >= 1.5 and tail_instrument_rms > 0.01,
        "duration_seconds": round(mix_duration, 3),
        "vocal_duration_seconds": round(vocal_duration, 3),
        "duration_error_seconds": round(duration_error, 3),
        "vocal_peak": round(vocal_peak, 6),
        "instrument_peak": round(inst_peak, 6),
        "mix_peak": round(mix_peak, 6),
        "vocal_rms_dbfs": round(db(vocal_rms), 3),
        "instrument_rms_dbfs": round(db(inst_rms), 3),
        "mix_rms_dbfs": round(db(mix_rms), 3),
        "integrated_lufs": lufs,
        "sample_peak_dbfs": round(db(mix_peak), 3),
        "eight_bar_rms_dbfs": [round(value, 3) for value in section_db],
        "eight_bar_dynamic_range_db": round(section_range, 3),
        "final_24s_instrument_rms_dbfs": round(db(tail_instrument_rms), 3),
        "vocal_to_instrument_rms_db": round(db(vocal_rms / max(inst_rms, 1e-12)), 3),
        "estimated_post_gain_vocal_instrument_db": round(estimated_balance, 3),
        "active_section_balance_min_db": round(active_section_balance_min, 3),
        "active_section_balance_max_db": round(max(section_balance_db), 3) if section_balance_db else None,
        "vocal_instrument_correlation": round(correlation, 6),
        "sample_rate": 44100,
    }
    out_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
