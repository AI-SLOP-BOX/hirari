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
| 15 | `d2cd6cb` | Piano roll (read-only): neutral `parse_midi_notes` + `piano_roll_geometry`, track picker, note canvas with beat grid, velocity-shaded notes and playhead. |
| 16 | `8816fc5` | Piano roll editing: press a note to move it (right edge resizes), press empty space to add one, DELETE removes the selection; commits as one full note-list replace in an undo transaction. `PianoNote` now carries the full authoring contract (lyric/articulation/vibrato survive edits). |
| 17 | `0046be0` | Piano roll extras: QUANTIZE (snap selected track's note starts to the snap grid) and VEL-/VEL+ stepping for the selected note, both through the same full-replace commit. |
| 18 | `0e66830` | Piano roll chord tools: ROOT±/quality/length/inversion steppers + CHORD insert at the playhead (Slint interval/inversion math) and DELETE ALL for the selected track. |
| 19 | `dd2e60b` | Piano roll lyric editing: a text field for the selected note's lyric, committed on submit through the full note-list replace (length/NUL guarded like the engine). |
| 20 | `d473190` | Piano roll articulation stepper: ART-/ART+ step the selected note's articulation (0–127) through the shared full-replace commit. |
| 21 | `1d13eb4` | Keyboard shortcuts: Space toggles playback, L toggles loop via `App::on_shortcut` (framework withholds it while a text editor is focused). |
| 22 | `9a17457` | Progression insert: P preset cycle + PROG button placing four chords (preset roots/qualities, inversion, engine chord-symbol events) at the playhead. |
| 23 | `8516db9` | Articulation map: MAP id cycler plus a KS keyswitch stepper (off/60/down) committed via `set_midi_expression_map`. `ArrangeTrack` now carries the expression map. |
| 24 | `33d1e4e` | Articulation name editing: a text field for the MAP id's name, committed on submit through the same expression-map replace. |
| 25 | `6afe0ef` | Clip inspector: fade in/out and gain sliders for the selected audio clip, committing through the engine undo transactions. `ArrangeClip` now carries fade fractions and gain. |
| 26 | `ea7b834` | Plugin strip: bypass toggles, removal and add (built-in chain stepper) for the selected track's plugin chain. `ArrangeTrack` now carries plugin types/bypass. |
| 27 | `149b915` | Canvas text: the landed `draw_text`/`measure_text` APIs draw lane headers, clip names, measure numbers and piano pitch labels; the arrange geometry gains the 104px lane-header column. |
| 28 | `ac4ee9c` | Canvas wheel: plain wheel scrolls the arrange timeline, Ctrl/⌘-wheel zooms anchored at the pointer beat, via the landed `on_canvas_scroll`. |
| 29 | `e41996b` | Native file dialog: Open/Save use the landed async `NativeFileDialog`, with results delivered to the poll (the engine is not `Send`, so dialogs never touch it). |
| 30 | `1682eb4` | Dropped-file import: OS file drops (landed `FileDropped`) become clips on the lane under the drop point via the queued engine import, polled to completion. |
| 31 | `dc178c7` | Dirty-canvas redraw: arrange/piano canvases replay cached scenes when idle (playhead/zoom/scroll/selection/layout tracked); the waveform always animates. |
| 32 | `9cf12ae` | Canvas gestures: pinch zooms (pointer-anchored) and trackpad pan scrolls the arrange canvas via the landed `on_canvas_gesture`. |
| 33 | `b1e48c4` | Theme colors: arrange/piano canvas fills and grid strokes follow the landed `CanvasContext::theme()` instead of hard-coded RGB. |
| 34 | `45484f6` | Plugin parameters: a params strip (first 32) for the selected plugin with normalized sliders, wired to `get/set_plugin_parameter`. |
| 35 | `142bf3d` | Drum mode: DRUM/PIANO toggle filtering the roll to GM drum lanes (35–81) with `drum_lane_label` row labels. |
| 36 | `4c92f58` | Automation overlays (read-only): volume/pan points from the layout drawn as polylines through each arrange lane. |
| 37 | `8b79940` | Automation editing: AUTO mode turns lane gestures into point add/move (snap + value deltas) with DEL PT removal, committing via `set_automation_data` in an undo transaction. |
| 38 | `ceafc30` | Audio settings: sample-rate/buffer cyclers, reconnect, device count and readiness readout, wired to `apply_audio_config`/`try_reconnect_audio_device`. |
| 39 | `98a105f` | Device selection: catalog parsing plus cycle/select buttons applying `select_audio_device` at the current config values. |
| 40 | `e3a5127` | Plugin presets: SAVE/LOAD preset buttons using the native dialogs, executed on the poll via the pending-preset slot. |
| 41 | `2c33fde` | Master gain slider in the meter card, wired to `set_master_gain` and polled from `master_gain`. |
| 42 | `da12104` | Slider release callbacks: clip fade/gain sliders commit on release (one undo entry per gesture) via the landed `on_release`. |
| 43 | `3749990` | Position-aware canvas cursors: resize cursor over Audio/Vocal trim edges, pointer over clips/notes, via the landed per-region `canvas_cursor` (adds the `winit` dep for `CursorIcon`). |
| 44 | `6d658bf` | Canvas accessibility: arrange tracks/clips and piano notes declared as screen-reader regions via the landed `canvas_accessibility_items`. |
| 45 | `f32d822` | Fixed waveform canvas height (168px) via the landed fixed-frame sizing, so the waveform texture is no longer stretched. |
| 46 | `0815f1d` | Audio library: folder scan, search filter, per-asset preview and import to the piano track, via `scan_preview_audio` and the queued import. |
| E1 | `fc04f16` | Engine: BS.1770 gated integrated loudness + LRA (`loudness_analyzer`). |
| E2 | `aae9409` | Engine: I/O latency measurement (device+unit properties + buffer) stored per stream, exposed as `io_latency_seconds`, applied as recording-start compensation, and shown in the AmeUI audio card. |
| E3 | `4f6426f` | Engine: dedicated recording poll worker with a single-consumer input queue and captured drop errors. |
| E4 | `40e3314` | Engine: true-peak detection in the native brickwall limiter using Catmull-Rom interpolation. |
| E5 | `3218953` | Engine: 2× oversampling in the tube saturator using the existing all-pass oversampler. |
| E6 | (this work) | Engine: LUFS-target normalization for queued offline exports — the orchestrator now measures gated integrated LUFS and applies the delivery gain (peak-capped) instead of plain peak normalization when `normalize` is set. |

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
3. ~~**Text in the canvas**~~ — landed in AmeUI (`CanvasContext::draw_text`
   + `measure_text`, 2026-11-08) and adopted in slice 27 (lane headers,
   clip names, measure numbers, pitch labels). Canvas accessibility
   regions landed and were adopted in slice 44. The track list below
   the canvas remains for mixer controls.
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

## Next surface: piano roll (extras)

The arrange dock + mixer + project + transport are at parity (text
excepted), and the piano roll edits since slice 16. Quantize, velocity
stepping, chord insert and delete-all landed in slices 17–18.
Remaining for the roll: CC lanes (unwired vestiges in Slint too).
Beyond the roll: sample editor, notation, synth panel, OpenUtau
editor — each its own multi-slice surface. (Audio library landed in
slice 46; master gain in slice 41.)
Entry points:

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
