#!/usr/bin/env python3
"""Audit the complete, reproducible three-hour release bundle."""
import json
import pathlib
import sys
import wave


REQUIRED_FILES = (
    "aura_three_hour_chisa.mid",
    "aura_three_hour_chisa.midi.json",
    "aura_three_hour_chisa_vocal.wav",
    "aura_three_hour_surge.wav",
    "aura_three_hour_surge_extended.wav",
    "aura_three_hour_mix_cli.wav",
    "aura_three_hour_song.aura",
    "audio_verify.json",
    "song_quality.json",
    "join_quality.json",
    "mix_quality.json",
    "project_inspect.json",
    "project_manifest.json",
    "generate_three_hour_song.py",
    "build_cli_song_project.py",
    "check_song_quality.py",
    "analyze_three_hour_mix.py",
    "check_vocal_provenance.py",
    "run_three_hour_release.sh",
    "propose_mora_grouping.py",
    "propose_outro_hook.py",
    "json_to_midi_candidate.py",
    "check_audio_joins.py",
    "build_tail_midi.py",
    "tail_render_manifest.json",
    "lyrics.txt",
    "export_release_lyrics.py",
    "production_notes.md",
    "vocal_render_provenance.json",
    "remaining_work.md",
)


def read_json(root: pathlib.Path, name: str):
    return json.loads((root / name).read_text(encoding="utf-8"))


def is_pcm16(path: pathlib.Path) -> bool:
    try:
        with wave.open(str(path), "rb") as handle:
            return handle.getsampwidth() == 2 and handle.getcomptype() == "NONE"
    except (OSError, wave.Error):
        return False


def audio_shape(path: pathlib.Path):
    """Return the delivery shape needed to compare PCM variants."""
    try:
        with wave.open(str(path), "rb") as handle:
            return {
                "channels": handle.getnchannels(),
                "sample_rate": handle.getframerate(),
                "frames": handle.getnframes(),
            }
    except (OSError, wave.Error):
        return None


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: audit_three_hour_release.py RELEASE_DIR")
    root = pathlib.Path(sys.argv[1])
    missing = [name for name in REQUIRED_FILES if not (root / name).is_file()]
    audio = read_json(root, "audio_verify.json") if not missing else {}
    song = read_json(root, "song_quality.json") if not missing else {}
    joins = read_json(root, "join_quality.json") if not missing else {}
    mix = read_json(root, "mix_quality.json") if not missing else {}
    project = read_json(root, "project_inspect.json") if not missing else {}
    provenance = read_json(root, "vocal_render_provenance.json") if not missing else {}
    tail_manifest = read_json(root, "tail_render_manifest.json") if not missing else {}
    lyric_lines = [line for line in (root / "lyrics.txt").read_text(encoding="utf-8").splitlines() if line.strip()] if not missing else []
    pcm24 = audio_shape(root / "aura_three_hour_mix.wav")
    pcm16 = audio_shape(root / "aura_three_hour_mix_cli.wav")
    delivery_shape_match = (
        pcm24 is not None
        and pcm16 is not None
        and pcm24["channels"] == pcm16["channels"]
        and pcm24["sample_rate"] == pcm16["sample_rate"]
        and abs(pcm24["frames"] - pcm16["frames"]) <= 1
    )
    checks = {
        "required_files": not missing,
        "audio_gate": audio.get("ok") is True,
        "song_gate": song.get("ok") is True and song.get("vocal_notes", 0) >= 180,
        "join_gate": joins.get("ok") is True,
        "mix_gate": mix.get("ok") is True and mix.get("duration_seconds", 0) >= 180,
        "duration_consistency_gate": mix.get("duration_error_seconds", 99.0) <= 0.5,
        "loudness_gate": -16.0 <= mix.get("integrated_lufs", -99.0) <= -10.0
        and mix.get("sample_peak_dbfs", 0.0) <= -0.5,
        "arrangement_contrast_gate": mix.get("eight_bar_dynamic_range_db", 0.0) >= 1.5,
        "vocal_balance_gate": mix.get("estimated_post_gain_vocal_instrument_db", -99.0) >= -3.0,
        "local_masking_gate": mix.get("active_section_balance_min_db", -99.0) >= -8.0,
        "provenance_gate": bool(provenance.get("midi_vocal_sha256"))
        and bool(provenance.get("vocal_wav_sha256"))
        and provenance.get("vocal_note_count") == song.get("vocal_notes"),
        "project_gate": project.get("ok") is True and project.get("track_count", 0) >= 4,
        "release_is_pcm16": is_pcm16(root / "aura_three_hour_mix_cli.wav"),
        "delivery_shape_gate": delivery_shape_match,
        "tail_render_gate": tail_manifest.get("method") == "midi_tail_render_crossfade"
        and tail_manifest.get("extension_seconds", 0.0) > 0.0
        and tail_manifest.get("tail_start_beat", 0.0) > 0.0
        and tail_manifest.get("tail_builder", {}).get("notes", 0) > 0,
        "lyrics_export_gate": len(lyric_lines) == song.get("lyric_phrases", 0)
        and len(lyric_lines) >= 8,
    }
    result = {
        "ok": all(checks.values()),
        "release_dir": str(root),
        "checks": checks,
        "missing_files": missing,
        "vocal_notes": song.get("vocal_notes", 0),
        "project_notes": project.get("midi_note_count", 0),
        "duration_seconds": mix.get("duration_seconds", 0),
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    (root / "release_audit.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
