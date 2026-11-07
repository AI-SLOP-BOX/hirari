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
| 6 | (this work) | Click clip selection: a click (≤4px) selects via `clip_at`, a drag seeks. Selected clip draws brighter with a 2px border, matching Slint. Empty-lane clicks seek on release. Drag/trim mutations are still to come. |

## Parity status (AmeUI vs Slint arrange dock)

Done: transport, master meter, waveform, track names/kinds/clip counts,
lane/clip/marker/playhead geometry, click-to-seek, zoom (toolbar),
ruler-drag panning, click clip selection.

Not yet (suggested order):

1. ~~**Zoom/scroll**~~ — done in slice 5 (toolbar −/+/FIT + ruler pan;
   lane drags still seek). Remaining API gap: AmeUI's
   `AmeEvent::Scroll` is window-global (no canvas/position), so
   wheel-zoom over the canvas is still unavailable.
2. **Clip drag/trim** — selection landed in slice 6; the mutations are
   still to come: Slint `clip-move-commit`, `clip-trim-commit` with snap
   division. Route through the existing engine commands used by the
   Slint callbacks (`hirari-ui/src/ui/editor.rs`); MIDI clips
   (`clip.midi`) must stay non-mutating, as in Slint.
3. **Waveform peaks on clips** — Slint draws 128 peak pairs per clip
   (`clip_data.points`). Peak extraction is queued by Slint telemetry;
   AmeUI needs the same queue or a read-only synchronous pass.
4. **Text in the canvas** — lane headers and clip names. Needs a text
   path for host canvases (AmeUI widgets draw text internally; the host
   scene does not). Until then the track list below the canvas carries
   the names.
5. **Slint deletion gate** — once 1–4 reach parity, flip the default
   entry in `hirari-ui/src/main.rs`, delete the Slint stack, and drop
   the `slint-wgpu` feature wiring.

## Slint references worth re-reading

- Arrange surface + interactions: `hirari-ui/ui/arrange_dock.slint`
- Telemetry cadence and model sync: `hirari-ui/src/ui/telemetry_loop.rs`,
  `hirari-ui/src/ui/track_model_parts/track_model_sync.rs`
- Peak queueing: search `existing_waveforms` in `track_model_sync.rs`
