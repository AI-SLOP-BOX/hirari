# Remaining work

This bundle is reproducible and all automated release gates pass, but it is not
being called finished.

## Still open

- A revised lyric/melody pass needs a successful VoiSona MIDI import and a new
  Chis-A WAV before it can replace the current coherent render. Candidate text
  is deliberately not included in the release until its MIDI/WAV provenance
  matches.
- Human listening is still required for Japanese pronunciation, consonant
  timing, vocal masking, and whether the hook is memorable rather than merely
  repeated.
- The current vocal MIDI still contains 11 split small-kana mora pairs
  (`しゅ`, `きゃ`, `ちゃ`, `っ`, etc.); these are reported by
  `song_quality.json` and require a fresh Chis-A render before adoption.
- The current breath-placement rule intentionally drops some phrase-initial
  lyric tokens from the rendered MIDI (for example, `ぬれた` is emitted as
  `れた`); the next Chis-A pass must preserve those consonant onsets while
  moving the rest to an earlier note or a separate breath.
- The generator exposes `AURA_PRESERVE_PHRASE_ONSETS=1` for that next candidate
  pass; it is off by default so the signed current Chis-A render remains bound
  to its existing vocal MIDI.
- A non-adopted candidate pass is reproducible with
  `propose_mora_grouping.py` followed by `json_to_midi_candidate.py`; it
  merges five y-/small-vowel morae and leaves six sokuon tokens untouched.
- A second non-adopted outro candidate from `propose_outro_hook.py` adds a
  13-note final hook, fills the partial final section from 8 to 21 notes, and
  lands exactly at beat 396 (99 bars). It still needs a fresh Chis-A render.
- The combined candidate (mora grouping followed by the outro hook) has 410
  vocal notes, 6 remaining sokuon splits, 21 final-section notes, and the same
  beat-396 landing. It is the current preferred candidate, pending Chis-A.
- `run_three_hour_release.sh` now accepts a paired
  `AURA_THREE_HOUR_MIDI`/`AURA_THREE_HOUR_MIDI_JSON` override, so a freshly
  rendered candidate can enter the same provenance and release gates without
  overwriting the canonical generator output.
- Candidate override mode now refuses to run without a preverified
  `AURA_CHISA_PROVENANCE` file, preventing an old WAV from being silently
  rebound to a new MIDI; the refusal happens before the Surge render starts.
- The current automated contrast check measures 8-bar RMS; it does not judge
  arrangement taste, lyric meaning, or emotional delivery.
- The final partial vocal section contains only 8 notes and ends at beat 389;
  a future Chis-A pass should decide whether that sparse outro is expressive
  or underwritten.
- External-device, VST/AU/CLAP, and long-session stability verification remain
  outside this offline release loop.

## Verified in this bundle

- Full-length 216-second PCM16 mix and CLI audio verification; mix duration now matches the Chis-A render within 0.5 seconds
- Raw Surge output is retained alongside a documented extended accompaniment render. Because the external renderer is fixed at 192 seconds, the final 11 MIDI bars are rendered separately and crossfaded into the release tail instead of simply repeating the raw ending.
- 44.1 kHz project timeline aligned with the final mix
- MIDI quality, phrase-length, hook repetition, pitch range, and section span
- LUFS, sample peak, vocal balance, correlation, and 8-bar contrast
- Active 8-bar local masking balance gate (minimum -8 dB after mix gains)
- Automated extension-join gate at the calculated raw-render boundary and
  its preceding 24-second window (maximum normalized jump 0.05)
- `tail_render_manifest.json` records the dedicated MIDI-tail render and its
  calculated start beat, and the release audit verifies it.
- Chis-A WAV to vocal-MIDI provenance binding
- Aura project inspect/manifest and 874 sequential Rust tests
