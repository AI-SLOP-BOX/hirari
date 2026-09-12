#!/usr/bin/env python3
"""Reject MIDI generations that are musically sparse or mechanically repetitive."""

import json
import statistics
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: check_song_quality.py SONG.midi.json")
    path = Path(sys.argv[1])
    tracks = json.loads(path.read_text(encoding="utf-8"))
    vocal = next((track["notes"] for track in tracks if track["track_id"] == 3), [])
    if len(vocal) < 180:
        raise SystemExit(f"quality gate failed: only {len(vocal)} vocal notes")
    starts = sorted(float(note["start_beat"]) for note in vocal)
    pitches = [int(note["pitch"]) for note in vocal]
    lyrics = [str(note.get("lyric", "")) for note in vocal if note.get("lyric")]
    small_kana = set("ゃゅょぁぃぅぇぉっゎ")
    split_mora_pairs = []
    ordered_vocal = sorted(vocal, key=lambda item: float(item["start_beat"]))
    ordered_starts = [float(note["start_beat"]) for note in ordered_vocal]
    ordered_pitches = [int(note["pitch"]) for note in ordered_vocal]
    for index, note in enumerate(ordered_vocal):
        token = str(note.get("lyric", ""))
        if token in small_kana and index:
            previous = str(ordered_vocal[index - 1].get("lyric", ""))
            split_mora_pairs.append(previous + token)
    gaps = [b - a for a, b in zip(starts, starts[1:])]
    phrases = []
    current = []
    previous = None
    for note in sorted(vocal, key=lambda item: float(item["start_beat"])):
        start = float(note["start_beat"])
        token = str(note.get("lyric", ""))
        if previous is not None and start - previous > 1.5 and current:
            phrases.append("".join(current))
            current = []
        if token:
            current.append(token)
        previous = start
    if current:
        phrases.append("".join(current))
    if max(pitches) - min(pitches) < 7:
        raise SystemExit("quality gate failed: vocal range is too narrow")
    if gaps and max(gaps) > 16:
        raise SystemExit("quality gate failed: vocal has an excessive silent gap")
    if len(set(lyrics)) < 30:
        raise SystemExit("quality gate failed: lyric material is too repetitive")
    if len(phrases) < 8:
        raise SystemExit("quality gate failed: too few lyric phrases")
    if max(map(len, phrases)) > 28:
        raise SystemExit("quality gate failed: a lyric phrase is too dense to breathe")
    repeated_phrases = {phrase for phrase in phrases if phrases.count(phrase) > 1}
    if not repeated_phrases:
        raise SystemExit("quality gate failed: no memorable repeated phrase/hook")
    sections = {int(start // 32) for start in starts}
    if len(sections) < 3:
        raise SystemExit("quality gate failed: vocal does not span enough sections")
    final_section = max(sections)
    final_section_notes = [
        note for note in ordered_vocal if int(float(note["start_beat"]) // 32) == final_section
    ]
    print(json.dumps({
        "ok": True,
        "vocal_notes": len(vocal),
        "lyric_tokens": len(lyrics),
        "unique_lyric_tokens": len(set(lyrics)),
        "split_mora_pairs": split_mora_pairs,
        "split_mora_pair_count": len(split_mora_pairs),
        "lyric_phrases": len(phrases),
        "longest_phrase_chars": max(map(len, phrases)),
        "repeated_hook_phrases": sorted(repeated_phrases)[:4],
        "pitch_min": min(pitches),
        "pitch_max": max(pitches),
        "pitch_range": max(pitches) - min(pitches),
        "median_gap_beats": round(statistics.median(gaps), 3) if gaps else 0,
        "max_gap_beats": round(max(gaps), 3) if gaps else 0,
        "sections": sorted(sections),
        "last_vocal_end_beat": round(
            max(float(note["start_beat"]) + float(note.get("length_beats", 0.0)) for note in vocal), 4
        ),
        "final_section_note_count": len(final_section_notes),
        "section_note_counts": {
            str(section): sum(1 for start in starts if int(start // 32) == section)
            for section in sorted(sections)
        },
        "section_pitch_ranges": {
            str(section): max(
                pitch for pitch, start in zip(ordered_pitches, ordered_starts) if int(start // 32) == section
            ) - min(
                pitch for pitch, start in zip(ordered_pitches, ordered_starts) if int(start // 32) == section
            )
            for section in sorted(sections)
        },
    }, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
