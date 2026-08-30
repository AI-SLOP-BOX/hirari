#!/usr/bin/env python3
"""Create a more natural OpenUtau USTX from the simple Aura chorus draft."""

import re
import sys
import os
import csv
from pathlib import Path


NOTE_RE = re.compile(r"(^[ \t]+- position: )(?P<pos>\d+)(?P<body>.*?)(?=^[ \t]+- position: |\Z)", re.M | re.S)
TONE_RE = re.compile(r"(^\s+tone: )(?P<tone>\d+)$", re.M)
PITCH_RE = re.compile(
    r"(?ms)^[ \t]+pitch:\n(?:^[ \t]+.*\n)*?^[ \t]+snap_first: (?:true|false)\n"
)
VIB_RE = re.compile(r"^\s+vibrato: .*?$", re.M)


def tuned_pitch(index, tone, next_tone, scoop_amount):
    # Use scoops only where a singer would actually need an attack gesture.
    # Applying the same dip to every note is what makes a tuned UTAU phrase
    # sound like a machine.  Keep stepwise notes nearly straight and reserve
    # the larger gesture for leaps.
    leap = next_tone - tone
    diagnostic = os.environ.get("AURA_TUNING_DIAGNOSTIC") == "1"
    scoop_cap = 16 if diagnostic else 8
    settle_cap = 16 if diagnostic else 8
    scoop = 0
    if abs(leap) >= 3:
        scoop = -max(2, min(scoop_cap, int(scoop_amount))) if leap > 0 else max(1, min(scoop_cap // 2, int(scoop_amount * 0.45)))
    settle = max(-settle_cap, min(settle_cap, leap * (2.0 if diagnostic else 1.5)))
    # USTX pitch deviation is expressed in cents, not semitones.
    scoop_cents = int(scoop * 100)
    settle_cents = int(settle * 100)
    return (
        "    pitch: {data: [{x: 0, y: %d, shape: io}, "
        "{x: 90, y: 0, shape: io}, {x: 330, y: %d, shape: io}, "
        "{x: 450, y: %d, shape: io}, {x: 480, y: 0, shape: io}], snap_first: true}\n"
        % (scoop_cents, settle_cents, settle_cents // 2)
    )


def tune(text, scoop_amount=8, vibrato_depth=10, vibrato_length=28, dynamics=0.6, consonants=0.5):
    # Match the singer identifier installed by OpenUtau on macOS.  Keeping
    # this normalization here prevents old Aura drafts from opening as
    # [Missing] even when the Teto voicebank is installed.
    # Preserve the singer identifier from the source USTX. OpenUtau installs
    # can expose different display names for the same voicebank; replacing
    # it here can turn a valid installed singer into [Missing].
    blocks = list(NOTE_RE.finditer(text))
    canonical_lyrics = list("ひかりほどけるよるにきみとみつけたこえ")
    canonical_tones = [60, 62, 64, 64, 62, 60, 62, 64, 65, 64, 67, 65, 64, 62, 60, 62, 64, 65, 64]
    tones = []
    for index, block in enumerate(blocks):
        match = TONE_RE.search(block.group("body"))
        tone = int(match.group("tone")) if match else 60
        embedded = re.search(r"NoteNum=(\d+)", block.group("body"))
        if (tone == 0 or embedded) and index < len(canonical_tones):
            tone = canonical_tones[index]
        tones.append(tone)

    replacements = []
    for index, block in enumerate(blocks):
        body = block.group("body")
        tone = tones[index]
        next_tone = tones[index + 1] if index + 1 < len(tones) else tone
        if index < len(canonical_lyrics):
            body = re.sub(r"(^\s+lyric: ).*$", r"\g<1>" + canonical_lyrics[index], body, count=1, flags=re.M)
        body = re.sub(r"(^\s+tone: )\d+\s*$", r"\g<1>" + str(tone), body, count=1, flags=re.M)
        body = PITCH_RE.sub(tuned_pitch(index, tone, next_tone, scoop_amount), body, count=1)
        # Vibrato belongs on phrase endings and sustained notes, not every
        # quarter note.  The USTX draft has one-beat notes, so only use a very
        # late, shallow tail on selected cadence notes.
        is_cadence = index in {5, 9, 11, len(blocks) - 1}
        vib_len = int(vibrato_length) if is_cadence else 0
        vib_depth = int(vibrato_depth) if is_cadence else 0
        vibrato = "    vibrato: {length: %d, period: 190, depth: %d, in: 32, out: 28, shift: 0, drift: 0, vol_link: 0}" % (vib_len, vib_depth)
        body = VIB_RE.sub(vibrato, body, count=1)
        # Keep the USTX at version 0.9 compatibility.  Per-phoneme
        # expressions are not understood by older OpenUtau builds, so the
        # tuning pass stays in the portable pitch/vibrato fields.
        replacements.append((block.start("body"), block.end("body"), body))

    for start, end, body in reversed(replacements):
        text = text[:start] + body + text[end:]
    return text


def write_tuning_manifest(destination, entries):
    """Write a human-editable tuning map next to the UST/USTX file.

    OpenUtau remains the source of truth for rendering, while this small
    manifest makes every intentional pitch gesture visible without reverse
    engineering the USTX YAML.
    """
    manifest = destination.with_suffix(".tuning.csv")
    fields = [
        "index", "position", "lyric", "tone", "next_tone", "interval",
        "scoop_semitones", "settle_semitones", "vibrato_length", "vibrato_depth",
        "edit_hint",
    ]
    with manifest.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(entries)
    guide = destination.with_suffix(".tuning.md")
    lines = [
        f"# Tuning map: {destination.name}",
        "",
        "Open this UST/USTX in OpenUtau. The CSV mirrors the intentional tuning gestures:",
        "- `scoop_semitones`: attack dip/rise; reduce toward 0 for a straighter attack.",
        "- `settle_semitones`: early transition toward the next note; remove it on stepwise phrases.",
        "- `vibrato_*`: non-zero only on phrase cadences; set both to 0 for a clean sustain.",
        "",
        "| # | lyric | tone | next | scoop | settle | vibrato | edit hint |",
        "|---:|:---:|---:|---:|---:|---:|:---:|:---|",
    ]
    for entry in entries:
        vib = f"{entry['vibrato_depth']} / {entry['vibrato_length']}" if int(entry["vibrato_length"]) else "off"
        lines.append(
            f"| {entry['index']} | {entry['lyric']} | {entry['tone']} | {entry['next_tone']} | "
            f"{entry['scoop_semitones']} | {entry['settle_semitones']} | {vib} | {entry['edit_hint']} |"
        )
    guide.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return manifest, guide


def main():
    if len(sys.argv) < 3 or len(sys.argv) > 8:
        raise SystemExit("usage: tune_openutau_chorus.py INPUT.ustx OUTPUT.ustx [scoop vibrato_depth vibrato_length dynamics consonants]")
    source, destination = map(lambda p: Path(p).expanduser(), sys.argv[1:3])
    args = [float(value) for value in sys.argv[3:]]
    while len(args) < 5:
        args.append([8.0, 10.0, 28.0, 0.6, 0.5][len(args)])
    if destination.suffix.lower() == ".ust":
        # Classic UST remains readable by older OpenUtau builds. Keep the
        # same vocal phrase while encoding pitch, vibrato, velocity and
        # consonant timing in the legacy format.
        lyrics = ["ひ", "か", "り", "ほ", "ど", "け", "る", "よ", "る", "に", "き", "み", "と", "み", "つ", "け"]
        tones = [60, 62, 64, 64, 62, 60, 62, 64, 65, 64, 67, 65, 64, 62, 60, 60]
        voice_dir = os.environ.get("AURA_TETO_VOICE_DIR", "/Users/REDACTED/Library/Application Support/OpenUtau/Singers/KasaneTeto")
        lines = ["[#SETTING]", "Tempo=120", "ProjectName=Aura Teto Tuned", f"VoiceDir={voice_dir}", "", "[#VERSION]", "UST Version1.2", ""]
        for index, (lyric, tone) in enumerate(zip(lyrics, tones)):
            scoop = -max(1, int(args[0] if args else 8)) if index % 3 else -max(1, int((args[0] if args else 8) * 0.5))
            depth = int(args[1] if len(args) > 1 else 10)
            length = int(args[2] if len(args) > 2 else 28)
            velocity = int(55 + (args[3] if len(args) > 3 else 0.6) * 35 + (index % 3) * 3)
            leap = tones[index + 1] - tone if index + 1 < len(tones) else 0
            scoop = 0 if abs(leap) < 3 else (-max(2, min(8, int(args[0] if args else 4))) if leap > 0 else max(1, min(4, int((args[0] if args else 4) * 0.45))))
            piches = ",".join(str(v) for v in (scoop, 0, max(-6, min(6, int(leap * 1.5))), 0, 0))
            cadence = index in {5, 9, 11, len(lyrics) - 1}
            vbr = f"{length},190,{depth},32,28,0" if cadence else "0,190,0,0,0,0"
            lines.extend([
                f"[#{index:04d}]", "Length=480", f"Lyric={lyric}", f"NoteNum={tone}",
                f"Intensity={velocity}", "Modulation=0", f"PreUtterance={max(0, 60 - int((args[4] if len(args) > 4 else 0.5) * 40))}",
                "VoiceOverlap=0", f"Piches={piches}", f"VBR={vbr}", "",
            ])
        lines.append("[#TRACKEND]")
        destination.write_text("\n".join(lines) + "\n", encoding="utf-8")
    else:
        destination.write_text(tune(source.read_text(encoding="utf-8"), *args[:5]), encoding="utf-8")
        source_text = destination.read_text(encoding="utf-8")
        blocks = list(NOTE_RE.finditer(source_text))
        tones = []
        lyrics = []
        for block in blocks:
            tone_match = TONE_RE.search(block.group("body"))
            lyric_match = re.search(r"^\s+lyric: (.*)$", block.group("body"), re.M)
            tones.append(int(tone_match.group("tone")) if tone_match else 60)
            lyrics.append(lyric_match.group(1).strip() if lyric_match else "")
        entries = []
        for index, block in enumerate(blocks):
            tone = tones[index]
            next_tone = tones[index + 1] if index + 1 < len(tones) else tone
            leap = next_tone - tone
            scoop = 0 if abs(leap) < 3 else (-max(2, min(8, int(args[0]))) if leap > 0 else max(1, min(4, int(args[0] * 0.45))))
            diagnostic = os.environ.get("AURA_TUNING_DIAGNOSTIC") == "1"
            settle = max(-16 if diagnostic else -8, min(16 if diagnostic else 8, int(leap * (2.0 if diagnostic else 1.5))))
            scoop = scoop * 100
            settle = settle * 100
            cadence = index in {5, 9, 11, len(blocks) - 1}
            entries.append({
                "index": index,
                "position": re.search(r"^\s*- position: (\d+)", block.group(0), re.M).group(1),
                "lyric": lyrics[index],
                "tone": tone,
                "next_tone": next_tone,
                "interval": leap,
                "scoop_semitones": int(scoop / 100),
                "settle_semitones": int(settle / 100),
                "vibrato_length": int(args[2]),
                "vibrato_depth": int(args[1]) if cadence else 0,
                "edit_hint": "cadence: keep gentle vibrato" if cadence else ("large leap: check scoop" if abs(leap) >= 3 else "step: keep mostly straight"),
            })
        write_tuning_manifest(destination, entries)
    print(destination)


if __name__ == "__main__":
    main()
