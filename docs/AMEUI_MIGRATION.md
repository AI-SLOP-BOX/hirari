# Slint → AmeUI migration

Hirari's shipped UI is Slint (`hirari-ui/src/slint_ui.rs`, default build).
The migration target is AmeUI (`hirari-ui/src/ameui/mod.rs`, behind the
opt-in `ameui` feature). This is a strangler migration: the Slint surface
stays the default entry until the AmeUI port reaches parity, at which
point the Slint stack is deleted. Do not add new features to Slint-only
code; put shared logic in the neutral modules below so both surfaces use it.

## How to run and verify

```sh
cargo run -p hirari-ui --features ameui   # AmeUI shell (needs ../AmeUI path dep)
cargo test -p hirari-ui                   # default build; arrange-model tests live here
cargo fmt -p hirari-ui -- --check
```

Notes:

- `ame-ui` is a **path dependency** (`../AmeUI`). If that sibling checkout
  has uncommitted work that does not compile, the `ameui` feature build
  fails for reasons outside this repo. To verify Hirari's side in that
  case, check against a clean copy of AmeUI at its HEAD.
- Neutral-logic tests live in `hirari-ui/src/ui/arrange_model.rs` and run
  in the **default** build, so they stay verifiable even while the
  feature build is broken by the sibling repo. Keep new geometry/parsing
  there, not in `ameui/mod.rs` (which only compiles under the feature).

## Conventions (learned from slices 1–4)

- **Neutral model first**: anything both backends need (layout parsing,
  timeline/measure math, canvas geometry) goes in `ui/arrange_model.rs`
  with unit tests. `ameui/mod.rs` only draws and forwards engine values.
- **Engine on the UI thread**: `HirariCore` is not `Send`. Poll it in
  `on_event` (ticker only sends `"tick"`), never from background threads.
- **Poll cadence**: transport/meters every ~33 ms tick; layout-derived
  state (tracks, markers, ruler, timeline end) every 10th tick.
- `State<T>` requires `T: Clone + PartialEq + Send + 'static`.
- The arrange canvas draws **vector shapes only** (no text — vello scenes
  have no text layout). Lane names stay in the track list view below it.

## Slice log

| Slice | Commit | What landed |
|---|---|---|
| 1 | `087ec194` | AmeUI shell boots a real `HirariCore`: transport, master meter, waveform Canvas (reusable-texture bridge), track list. |
| 2 | `9c9e821e` | Neutral `ui/arrange_model.rs` extracted from Slint paths (time signatures, timeline end, measure starts, MIDI span grouping, layout/marker parsing, lane colors). Fixed a timeline-end overshoot (was +4 bars). AmeUI track list now uses `parse_arrange_tracks`. |
| 3 | `618da46` | Arrangement Canvas: ruler, beat grid, measure lines, lanes, clips, markers, playhead — fit-to-width from `arrange_canvas_geometry`. |
| 4 | `618da46` | Click/drag on the arrange canvas seeks the playhead (`beat_for_x` + `beats_to_samples`/`set_playhead`; seek only, playback untouched). |
| 5 | `2181009` | Zoom (toolbar −/+/FIT, ×/÷1.25, 1–8×) and ruler-drag panning. `arrange_canvas_geometry` now takes zoom + scroll and clamps both. Wheel-zoom is still unavailable (see API gap below). |
| 6 | `f06a387` | Click clip selection: a click (≤4px) selects via `clip_at`, a drag seeks. Selected clip draws brighter with a 2px border, matching Slint. Empty-lane clicks seek on release. |
| 7 | `ff7e2d6` | Clip drag-move: drag a (non-MIDI) clip to move it with a live preview, commit on release through the engine undo transaction (`move_region` / `move_region_sync_group`, 0.25-beat snap). `ArrangeClip` now carries `sync_group`. |
| 8 | `12c36d8` | Clip edge-drag trim: Audio/Vocal clip edges trim with live preview, commit on release (`set_region_trim`, fraction-clamped like the Slint dock). `ArrangeClip` now carries the trim window. Edge zones come from `trim_edge_at`. |
| 9 | `6a2d1eb` | Clip waveform peaks: async decode queue (8 in flight, generation-guarded, failure memo, revision-cached generation) feeding 128 trim/reverse-windowed peak pairs per clip, drawn like the Slint dock. `ArrangeClip` now carries `reverse`. |
| 10 | `f3eb1c5` | Snap division picker: toolbar button cycles 1/1–1/32 (step = 4/division) for move/trim drags, replacing the hardcoded 1/16. |
| 11 | `d1ab781` | Edit lock: toolbar LOCK toggles Slint `is_locked` for the arrange surface — while on, clip drags select only (seek/pan unaffected). |
| 12 | `d605673` | Mixer strips: the track list becomes per-track fader/pan sliders plus mute/solo/arm toggles, wired to the same engine setters as the Slint mixer. `ArrangeTrack` now carries the mixer state. |
| 13 | `3d5b596` | Project bar: Open/Save/New via `rfd` dialogs and `load_project`/`save_project`/`new_project`, with path memory and selection reset on project change. |
| 14 | `1f50643` | Transport extras: Loop and metronome toggle buttons, polled from `is_loop_enabled`/`is_metronome_enabled`. Adds the missing `HirariCore` metronome forwarders in the bridge. |
| 15 | (this work) | Piano roll (read-only): neutral `parse_midi_notes` + `piano_roll_geometry`, track picker, note canvas with beat grid, velocity-shaded notes and playhead. Editing gestures are next. |

## Parity status (AmeUI vs Slint arrange dock)

Done: transport, master meter, waveform, track names/kinds/clip counts,
lane/clip/marker/playhead geometry, click-to-seek, zoom (toolbar),
ruler-drag panning, click clip selection, clip drag-move, edge-drag trim,
clip waveform peaks, mixer strips (fader/pan/mute/solo/arm), edit lock,
project open/save/new, loop/metronome toggles.

Not yet (suggested order):

1. ~~**Zoom/scroll**~~ — done in slice 5 (toolbar −/+/FIT + ruler pan;
   lane drags still seek). Remaining API gap: AmeUI's
   `AmeEvent::Scroll` is window-global (no canvas/position), so
   wheel-zoom over the canvas is still unavailable.
2. ~~**Waveform peaks on clips**~~ — done in slice 9 (queue in
   `poll_waveforms`, trim/reverse-windowed draw). The snap picker
   landed in slice 10. Still open from the edit area: fade/gain
   editing, which Slint does outside the arrange dock.
3. **Text in the canvas** — lane headers and clip names. Blocked on the
   framework: AmeUI's host canvas API exposes textures and vector
   shapes only; `draw_text` is crate-internal (`widgets/render.rs`), so
   host apps cannot draw text into a canvas scene. Filed as the first
   item of the Hirari section in AmeUI's `REQUESTS.md` (2026-11-08, plus
   follow-ups: cursor, double-click, hover spec, theme, a11y, file
   drop, canvas sizing). Until AmeUI exports a host text helper, the
   track list below the canvas carries the names. (The arrange dock's
   edit lock landed in slice 11.)
4. **Slint deletion gate** — once the above reach parity, flip the
   default entry in `hirari-ui/src/main.rs`, delete the Slint stack,
   and drop the `slint-wgpu` feature wiring.

## Framework discoveries (verified 2026-11-08)

Already available in AmeUI, no request needed: native menu bar
(`App::native_menus` / `on_native_menu_action`), window-global keyboard
(`AmeEvent::Key` via `on_event`), IME preedit/commit events, structured
same-window drop, window title syncing, close-request and shortcut
hooks. The piano-roll slice 16 work is uncommitted in `ameui/mod.rs`
(add/move/resize/delete via full note-list replace).

## Next surface: piano roll (editing)

The arrange dock + mixer + project + transport are at parity (text
excepted), and the piano roll is read-only since slice 15. Remaining for
the roll: add/move/resize/delete gestures. Entry points:

- Data: `core.midi_notes_json()` (timeline-sorted; see
  `hirari-core-bridge/src/runtime/hirari_core_arrangement_midi_editing.rs`)
  and the note-editing engine calls in the same file.
- Slint reference: `hirari-ui/ui/model/midi_model.slint`,
  `hirari-ui/ui/hirari_studio_parts/editor_views.slint`; note grouping
  semantics already live in `arrange_model::group_midi_spans`.
- Suggested order inside the slice: neutral note parser + geometry in
  `arrange_model.rs` (default-build tests), read-only canvas, then
  click-to-add / drag gestures, then velocity editing.

## Slint references worth re-reading

- Arrange surface + interactions: `hirari-ui/ui/arrange_dock.slint`
- Telemetry cadence and model sync: `hirari-ui/src/ui/telemetry_loop.rs`,
  `hirari-ui/src/ui/track_model_parts/track_model_sync.rs`
- Peak queueing: search `existing_waveforms` in `track_model_sync.rs`
