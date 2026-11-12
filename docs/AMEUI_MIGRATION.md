# Slint → AmeUI migration (completed)

Hirari's shipped UI was Slint; the migration target was AmeUI. The
strangler-pattern pivot completed — the Slint surface is deleted in the
commit series that closes this document's slice log. The `slint` and
`slint-build` crates are gone from `Cargo.lock`. The `ameui` cargo feature
that used to opt into the replacement surface is gone with it; the shipped
shell is the AmeUI surface by default.

The migration log below is kept for history. Slices 1–53 are unchanged;
slices 54+ record the AmeUI-now-default flip and the workflow ports that
were carried through to parity, after which the Slint stack was removed.

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
| 47 | `1f3e018` | Adopts the landed `ame_ui::CursorIcon` re-export for the position-aware canvas cursor; the direct `winit` dependency is dropped. |
| 48 | `c73c14f` | Audio library rows virtualized with the landed variable-height `VirtualList` (asset id as `StableKey`). |
| 49 | `e4e16ca` | Adopts the landed `CanvasContext::elide_text`; the local elision helper is deleted. |
| 50 | `e26eb10` | Double-click (landed `click_count`) splits the arrange clip under the cursor via `split_region`. |
| 51 | `0bd0d1d` | Arrange clip labels drawn with the landed `draw_text_in_rect` (elision + vertical centering from the framework). |
| E1 | `fc04f16` | Engine: BS.1770 gated integrated loudness + LRA (`loudness_analyzer`). |
| E2 | `aae9409` | Engine: I/O latency measurement (device+unit properties + buffer) stored per stream, exposed as `io_latency_seconds`, applied as recording-start compensation, and shown in the AmeUI audio card. |
| E3 | `4f6426f` | Engine: dedicated recording poll worker with a single-consumer input queue and captured drop errors. |
| E4 | `40e3314` | Engine: true-peak detection in the native brickwall limiter using Catmull-Rom interpolation. |
| E5 | `3218953` | Engine: 2× oversampling in the tube saturator using the existing all-pass oversampler. |
| E6 | `e89148d` | Engine: LUFS-target normalization for queued offline exports — the orchestrator now measures gated integrated LUFS and applies the delivery gain (peak-capped) instead of plain peak normalization when `normalize` is set. |
| E7 | `1d73b48` | Engine: project-end freeze now renders a 2 s processor tail beyond the last region, and unfreezing resets the live processors so stale delay/reverb state cannot leak back. |
| E8 | `efff559` | Engine: external plugin latency now reaches PDC. The mailbox ABI (v6) carries `plugin_latency_samples`; workers query it per format (CLAP latency extension, AU `kAudioUnitProperty_Latency`, VST3 `getLatencySamples`), the host captures it at READY, and the chain node reports it to the PDC solve. |
| E9 | `c985f37` | Engine: sidechain audio now reaches sandbox plugins. Mailbox ABI v7 carries stereo sidechain arrays; the block host injects the gate handle per track pass, the chain stamps (gate, track, node index) on sandbox nodes, the host copies the published source (with link level) into the mailbox, and the VST3 adapter feeds bus 1. CLAP/AU adapters ignore the arrays (silence). |
| E10 | `60b06a5` | Engine: parameter discovery probe. The worker grew a one-shot `--params` mode that loads a plugin and enumerates parameters per format (CLAP `clap.params`, AU parameter list/info, VST3 edit controller), printing one JSON line; the bridge exposes it as `probe_plugin_parameters_json`. |
| E11 | `478f0fc` | Engine: sandbox parameter discovery is wired into the chain. Sandbox nodes cache probe results (seeding the value cache with defaults), so `get_parameter_count`/names, the UI snapshot and automation lanes now see external plugins; `set` already flowed through the mailbox. |
| E12 | (this work) | Engine: AU sidechain delivery. The worker probes `kAudioUnitProperty_BusCount` on input; units with a second bus get a sidechain pull callback fed from the mailbox arrays (silence when unlinked). |
| P1 | `b524bed` | Plugin: VST3 fixture compatibility now covers Surge XT effects plus the Vital instrument path (MIDI note-on produces audible output). |
| R1 | `fad1332` (AmeUI) | Filed Hirari pt.6 requests: TimelineView controlled zoom/offset binding, marker/loop/selection edit events, and beat/bar ruler grids. |
| 52 | `75543f0` | TimelineView adopted as the arrange ruler (beats-domain, controlled zoom/offset, header indent 104, markers, scrub seeks the engine playhead). The canvas keeps lanes/clips. |
| F1 | `c1e6599` | Fix: the arrange pinch/pan handler lived in the inherent impl where nothing could call it; AmeUI dispatches `on_canvas_gesture` to the App trait, so it now overrides there. Caught by the dead_code warning. |
| F2 | `fd0a254` | Public hosts (forks, GitHub Actions) cannot resolve the private `ame-ui` path dependency. `scripts/ensure_ameui_stub.sh` creates a manifest-only placeholder derived from `Cargo.lock`; Actions build the legacy Slint backend against it (lockfile strictness stays on the internal Linux CI); `scripts/build_app.sh` gained `HIRARI_UI_FEATURES` for forks. |
| F3 | `3d3168a` | Moufu integration test hardened for the xvfb build fleets (Ping/pull-echo tolerance in the commit slot; 15 s deadlines). It had been intermittently failing on the internal Linux CI at EC083b2 and since. |
 53 | `87f4bb4` | AmeUI home page: template starters (`INIT_*` parity), recent projects list, opens/saves/hides the workspace; recent-project open uses the existing native-dialog pending path. |
| 57 | `8a9b06f` | AmeUI Moufu Hub card (P5): address field persisted, CONNECT/DISCONNECT/REFRESH, entity share/receive/unlink, read-only linked preview via shared `ui_shared::moufu_preview`; publisher snapshot publish through shared `ui_shared::moufu_layout` (no recording-input channel tags yet — engine/model gap). |
| 59 | `1bd92d1` | AmeUI video monitor slice (P17 part 1): VIDEO card with LOAD VIDEO + SHOW/HIDE toggle; control-rate poll tracks playhead revision; texture uploaded on the draw path like the waveform. |
| 61 | `337f762` | AmeUI OpenUtau vocal score card (P13): OPEN USTX / LAUNCH OPENUTAU (macOS `open -a`) / PICK RENDER / IMPORT VOCAL — engine-driven import through the poll, adds Vocal track + region. |
| 67 | `4e99352` | **Slint stack deleted.** `hirari-ui/ui/**/*.slint`, `hirari-ui/src/ui/`, `slint_ui.rs`, `slint_ui_parts/`, `moufu.rs`, `build.rs`, the `slint`, `slint-build`, `raw-window-handle`, `wgpu`, `bytemuck`, `egui`, `egui-wgpu` optional workspace dependencies, and the `slint-wgpu`/`ameui` cargo features (`ameui` is now the only build path). Linux CI gates on `cargo check --workspace --locked` and `cargo test --workspace --locked`; GitHub Actions checks only the engine crates with a manifest-only placeholder. |
| 70 | see below | AmeUI comping card (P10): take list from `comping_snapshot_json`, crossfade slider wired to `set_comp_crossfade_normalized`, per-take SELECT button (`select_comp_take`), shared with the arrange track selection. `ui_shared::arrange_model` now parses `CompTake` / `parse_comping_takes`. |
| 77 | (this commit) | AmeUI NOTATION canvas: grand-staff renderer for the selected MIDI/Instrument track — two 4/4 systems, barlines from `measure_starts`, diatonic noteheads (filled/hollow, stem by rule), ledger lines, `#` accidental markers, fontless glyphs only. NOTATION button in the project bar toggles the view (frame 240px). |
| 78 | discuss below | AmeUI CONTROL ROOM card: DIM checkbox + dB slider wired to set_control_room_dim / set_control_room_dim_db; TALKBACK checkbox + gain slider wired to set_control_room_talkback; state polled from engine each 10 ticks (dimmed()/dim_db()/talkback_enabled()). |
| 79 | (this commit) | Chord track integration: CHORD piano-roll button now calls add_chord_event alongside inserting the triad; new CHORD LANE card lists engine-persisted chord events (beat tick/960, name, stacked pitch classes) with per-event DEL and CLEAR ALL; both reads go through the bridge (chord_track_json / remove_chord_events_range / clear_chord_track). |
| 76 | `ecd271f` | AmeUI STEP SEQ card (P12, engine ext): track-scoped pattern editing wired through the new bridge APIs `step_sequencer_patterns_json`/`set_step_sequencer_patterns_json`; engine validation relaxed for track-scoped patterns (region_id == 0) with full uniqueness/resolution/feel checks; six lane rows with −/+ unique pitch; per-page 16-step grid toggles Note On/Off through normal Undo transactions; RESOLUTION 4/8/16/32/64 and FEEL Straight/Triplet/Dotted persist with the project Save. |
| 75 | `a3b935d` | AmeUI MIDI LEARN panel (P11): project-bar toggle, ARM MACRO 1–8 buttons, INPUT gate, INSERT RECENT MIDI via the ported `ameui/midi_learn.rs` (CoreMIDI poll, retrospective buffer, CC learn via add_midi_learn_mapping). |
| 74 | `6e48592` | AmeUI markers card: ADD MARKER at playhead, beat/label/delete rows, reads `markers_json` via engine. |
| 73 | `6e0464e` | AmeUI vocal-tuning row wired (P13): SCOOP/VIBRATO/DYNAMICS/CONSONANTS sliders + APPLY TUNING button calls set_openutau_tuning; import poll now registers the vocal via register_openutau_vocal after the region is created. |
| 71 | `2e1a9c2` | AmeUI audio edit card inline extended (slice 71): FADE IN/FADE OUT normalized sliders, LOOP cycler (1x->2x->4x), DUPLICATE at playhead, DELETE region button — all engine-driven with layout refresh |
| 69 | `cbe3844` | AmeUI flex-pitch card (P8): PRESERVE PITCH (WSOLA) checkbox wired to `set_region_pitch_preserve_warp`, SPEED slider 0.5-2.0x wired to `set_region_warp_ratio`, PITCH slider wired to `set_region_pitch_semitones` (value now surfaced from the layout), SYNC GROUP text field with apply, and QUANTIZE SYNC GROUP (strength slider, 1/4–1/64 × straight/triplet/dotted grid cycle, swing), all un-region-checked. The layout model now parses `warp_ratio` and `pitch_preserve_warp` for full reverse fidelity. |
| 68 | see below | AmeUI track stacks card (P10): CREATE FROM SELECTED (upsert via members), ADD SELECTED (add_track_stack_member), gain slider wired to set_track_stack_gain, COLLAPSE checkbox wired to set_track_stack_collapsed, DELETE STACK wired to delete_track_stack. `ui_shared::arrange_model` now parses `track_stacks_json` and exposes `parse_track_stacks`. State uses id-selected primitive with JSON-driven refresh on each poll tick. |
| 66 | `d2e4d9a` | AmeUI audio edit card (P14 partial): REVERSE toggle, PITCH-/PITCH+ stepping from engine-published pitch_semitones (layout now parses it), SPLIT at playhead. |
| 63 | `2c9537e` | AmeUI MusicXML export button (P15 partial): engine `export_musicxml` via save dialog, `.musicxml` extension, poll pipeline. |
| 62 | `160fee4` | AmeUI sends card (P4): source/bus cycler, gain slider with live apply (`set_send_route`), PRE/POST toggle, enable; state refreshed from `audio_routes_json()` at 10 Hz when the panel is open. |
| 60 | `5153234` | AmeUI TEMPO card (P19 partial): BPM-/BPM+, TAP TEMPO (0.2-3 s deltas, EMA), time-signature cycle (4/4 3/4 6/8 7/8), engine tempo-map event list with add/remove and ramp/step toggle. |
| 58 | `608e9e9` | AmeUI MASTER loudness card (P7): integrated LUFS + true-peak L/R polled via `get_master_loudness()`, gain and −14 LUFS/−1 dBTP delivery caption. |
| 56 | `281c610` | AmeUI diagnostics/SYSTEM card (P2): health line, dsp_load %, drops/overruns/failures/watchdog, overflow flag, RECOVER (`recover_sandboxed_plugins`), silent-fallback guidance. Replaces part of the Slint diagnostics page. |
| 55 | (commit hash) | AmeUI export card (P1 from SLINT_DELETION.md): RENDER (queue loaded mix with engine native format code + effect-tail presets 0/1/2/5/10/30 s), STEMS (folder picker arms `stem_batch_prepare`; the poll settles state 3 and advances the batch like the Slint telemetry), MIDI (`.mid` save via native dialog, `export_midi_file`), CANCEL (`cancel_render`), render progress caption polled every tick. Moves `ui_shared::render_ops` (stem_step, prepare_rejected, kick_stem, render_tail_seconds, render_format_label, cancellation_status) and `ui_shared::render_path` (default_render_output_path, unix_time_millis) out of Slint-only render.rs; Slint re-exports keep parity. |
| 54 | `9e436cf` | AmeUI becomes the default UI: the shipped `default` feature now enables `ameui`, with the Slint surface optional behind `slint-wgpu` (`slint`/`slint-build`/`raw-window-handle` are conditional deps; `build.rs` early-returns when `CARGO_FEATURE_SLINT_WGPU` is unset; `pub mod ui`/`slint_ui`/`moufu` are `cfg`-gated). Backends stay source-compatible: backend-neutral measure/geometry/template/recent-project code moved to `ui_shared` (`arrange_model`, `gpu_canvas`, `plot_raster`, `template_starter`, `recent_projects`). `scripts/build_app.sh` advertises `ui=ameui`. |

## Parity status (AmeUI vs Slint arrange dock)

Done: transport, master meter, waveform, track names/kinds/clip counts,
lane/clip/marker/playhead geometry, click-to-seek, zoom (toolbar),
ruler-drag panning, click clip selection, clip drag-move, edge-drag trim,
clip waveform peaks, mixer strips (fader/pan/mute/solo/arm), edit lock,
project open/save/new, loop/metronome toggles, flex-pitch edits
(preserve-pitch/speed/pitch/sync-group/quantize), track stacks
(create/add/gain/collapse/delete), comping take list with engine-driven
crossfade and select, markers add/delete, audio edits (reverse/pitch/
split/fade in/fade out/loop/duplicate/delete), vocal-tuning editing,
and send routing (a flexible plug for routing cards).

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

## Upstream continuity (2026-11-09)

The AmeUI side may get a new maintainer, so API conventions can shift
(release cadence, widget patterns, even the `.ame` idioms). Until the
new owner settles, treat freshly landed APIs as volatile: verify every
adoption against a clean AmeUI HEAD, keep the neutral layer
(`ui/arrange_model.rs`) thin and backend-agnostic, and record adopted
status in `AmeUI/REQUESTS.md` so a convention change is visible in the
diff before Hirari follows it.

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
