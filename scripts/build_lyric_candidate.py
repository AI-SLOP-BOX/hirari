#!/usr/bin/env python3
"""Create a non-canonical lyric candidate without changing note timing."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

PHRASES = [
    "しゅうでんのまどにまちがほどける", "ポケットのきっぷまだあたたかい",
    "きょうのことばをのみこんだまま", "ぬれたホームでひとりゆく",
    "あのひとつのこえをおもいだす", "さよならだけがうまくいえない",
    "それでもあさはまどをひらいて", "しらないそらをそっとみあげ",
    "わすれないでといえなかった", "きみのとなりでわらいたかった",
    "とおりすぎるひかりをつかむ", "このてのなかにのこすもの",
    "きのうまでのぼくをほどいて", "あたらしいくつであるきだす",
    "もしもいつかまたあえるなら", "こんどはちゃんとつたえる",
    "まよいながらもここまできたよ", "こわれたとけいをうごかすように",
    "きみがくれたひをだきしめて", "もういちどだけうたおう",
    "ほどけたこころをむすぶから", "あめのあとにはにじがひかるよ",
    "あたらしいひかりをつかむよ", "このてにあしたをのこすよ",
    "きのうのぼくをほどいてゆく", "あたらしいそらへあるきだす",
    "いつかきみにまたあえるなら", "こんどはきみにつたえるよ",
    "しゅうでんのまどにあさがひらいた", "きみのこえがあさをつれてくる",
]


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: build_lyric_candidate.py INPUT_JSON OUTPUT_JSON OUTPUT_MID")
    source, output_json, output_mid = map(Path, sys.argv[1:])
    document = json.loads(source.read_text(encoding="utf-8"))
    vocal = next(track for track in document if int(track["track_id"]) == 3)
    notes = sorted(vocal["notes"], key=lambda item: float(item["start_beat"]))
    groups: list[list[dict]] = []
    for note in notes:
        if not groups or float(note["start_beat"]) - float(groups[-1][-1]["start_beat"]) > 1.5:
            groups.append([])
        groups[-1].append(note)
    if len(groups) != len(PHRASES):
        raise SystemExit(f"expected {len(PHRASES)} lyric groups, got {len(groups)}")
    for phrase, group in zip(PHRASES, groups):
        if len(phrase) != len(group):
            raise SystemExit(f"phrase length mismatch: {phrase} ({len(phrase)}) != {len(group)}")
        for note, lyric in zip(group, phrase):
            note["lyric"] = lyric
    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    subprocess.run([sys.executable, str(Path(__file__).with_name("json_to_midi_candidate.py")),
                    str(output_json), str(output_mid)], check=True)
    print(json.dumps({"ok": True, "groups": len(groups), "notes": len(notes), "output": str(output_json)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
