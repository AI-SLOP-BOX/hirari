# Slint deletion gate

Goal: **zero Slint in the shipped workspace**, so the Slint license
(royalty-free/GPL-3.0-only) no longer applies anywhere in Hirari — sources,
`Cargo.toml`/`Cargo.lock`, build scripts (`build.rs` Slint compiler path),
docs, CI, and release attribution all go.

Mechanics of deletion are trivial once coverage exists. The hard work is
**porting the remaining workflow surfaces to AmeUI**, which is accounted
for below, one surface per line.

## Definition of done

- no `slint` / `slint-build` in any `Cargo.toml`, zero hits for
  `slint::` / `.slint` outside git history
- no `slint-wgpu` feature wiring; `hirari-ui` builds with `ameui` only
- `build.rs` no longer knows about `ui/*.slint`; `build/ui` directory is empty
- CI: Linux + GitHub verify `ameui`; legacy `slint-wgpu` path does not exist
- README/docs/`THIRD_PARTY_NOTICES`/licensing stop referencing Slint
- `./scripts/build_app.sh` and metadata say the binary ships AmeUI

## Disposition matrix

Legend: **PORT** = must reach AmeUI parity; **DELETE** = removed with the
Slint stack (demo/eye-candy/engineering input); **SHARED** = lives in
`ui_shared`, already backend-neutral.

### Backend-neutral shared (already outside Slint)

| Surface | File | Status |
|---|---|---|
| Arrange geometry/arrange tools | `ui_shared/arrange_model.rs` | done |
| WGPU plot textures | `ui_shared/gpu_canvas.rs` | done |
| CPU rasterizers | `ui_shared/plot_raster.rs` | done |
| Template starters | `ui_shared/template_starter.rs` | done |
| Recent projects | `ui_shared/recent_projects.rs` | done |

### Surfaces already in AmeUI (parity reached or close)

Genesis/home page, arrange dock (ruler/lanes/clips/markers/playhead),
mixer strips, piano roll (+quantize/chords/lyrics/articulation), audio
settings, device selection, master gain, plugin strip, parameters,
presets save/load, clip inspector (fade/gain), automation overlays+editing,
audio library, telemetry render panels, transport extras,
TimelineView. See slices 1–53 in `AMEUI_MIGRATION.md`.

### DEMO/EYE-CANDY — delete outright (no AmeUI port)

| File/tree | What it is | Notes |
|---|---|---|
| `ui/experimental/*.slint` (gate, views, hirari_ide, hirari_modular, live_loops, spectral_restoration, stage_launchpad) + `ui/experimental/` | experimental live-loop/launchpad/IDE shells | engine refs zero |
| `ui/assistant/neural_trainer.slint` | "neural trainer" assistant demo | engine refs zero |
| `ui/collaboration/cloud_bridge.slint`, `network_map.slint` | cloud collaboration demo | engine refs zero |
| `ui/spatial/*.slint` | vr_sandbox, sphere_telemetry, immersive_panner, stage_view | engine refs zero |
| `ui/system/atomic_clock.slint`, `ecosystem_pulse.slint`, `shield.slint` | decorative status eye-candy | engine refs zero |
| `ui/industrial_matrix.slint`, `ui/industrial_widgets.slint` | industrial matrix decoration | |
| `ui/sanctuary_shield*.png` (if only referenced by the above) | decoration | check references |
| `sanctuary_shield.png`/`sanctuary_shield_converted.png` | decoration | check references |

### PORT — the actual migration

Ordered by "engine-backed first" (usable as soon as they land in AmeUI),
approx. effort and status. Each item = one AmeUI slice.

| # | Surface | Slint refs | AmeUI status |
|---|---|---|---|
| P1 | Export dialog (render + LUFS target + freeze) | `ui/render.rs`, `dialogs/export_dialog.slint`, LUFS normalize queued export | **partial done** (`dea76d0`) — RENDER/STEMS/MIDI/CANCEL in AmeUI; LUFS QUEUED-DELIVERY intentionally deferred to a delivery dialog. |
| P2 | Diagnostics / health dashboard | `ui/diagnostics/*` | **done** (`281c610`) — SYSTEM card: health line, dsp_load %, drops/overruns/failures/watchdog, range overflow, silent-fallback hint, RECOVER |
| P3 | Full mixer / smart controls / console variants | `ui/mixer/*` (188 files large) `full_mixer.slint`, `smart_controls.slint`, `console_*` | partial (strips) |
| P4 | Routing (send matrix, pre/post, sends) | `ui/routing.rs`(?), `ui/routing_model*`, `ui/mixer/*` routing section | not started |
| P5 | Moufu link page | `moufu_link_page.slint` | **done** (`8a9b06f`) — address persistence + connect/refresh/share/receive/unlink, read-only preview, queue-safe revisioned publish |
| P6 | Preferences / setup pages | `ui/audio_settings.rs`, `ui/setup*`, `ui/preferences*` | partial (audio card) |
| P7 | Mastering page | `pages/mastering_page.slint` | **partial done** (`608e9e9`) — MASTER card with integrated LUFS, true peak L/R, gain, delivery target. Static module tiles + GPU curves not ported. |
| P8 | Flex pitch / flex edit | `ui/flex*`, `editor/*` | not started |
| P9 | Comping (take lanes) | `ui/comp*`, `editor/comping*` | not started (engine comping works, UI in Slint only) |
| P10 | Track stacks / global tracks | `pages/workflow_pages.slint` | not started |
| P11 | Step sequencer / drums | `pages/workflow_pages.slint`, `drums` view | engine has step seq; Mikboix UI only |
| P12 | MIDI learn | `pages/workflow_pages.slint` | engine refs (MIDI learn) in bridge |
| P13 | OpenUtau editor | `editor/openutau_editor.slint` | engine refs present (43); UI port needed |
| P14 | Sample editor (+adapter) | `editor/sample_editor_adapter.slint`, `editor/sample_editor.slint` | engine refs present |
| P15 | Notation / score | `editor/notation_hub.slint` | not started |
| P16 | Synth panel (/ sound_design) | `editor/synth_panel.slint` | not started |
| P17 | Video monitor | `media/video_monitor.slint` | engine video refs present |
| P18 | Advanced automation | `editor/advanced_automation.slint` | partial (overlay edit) |
| P19 | Global tempo editor | `editor/global_tempo_editor.slint` | not started |
| P20 | Spotlight command palette | `navigation/spotlight.slint` | not started |
| P21 | Workflow pages remainder (flex/global/midi learn) | `pages/workflow_pages.slint` | remaining |
| P22 | Assistant view (if any remains post-delete) | — | n/a |

### Legacy/build/packaging cleanup once the ports land

- delete `hirari-ui/ui/**/*.slint`, `hirari-ui/src/ui/`, `slint_ui_parts/`,
  `moufu.rs`, `slint_ui.rs`
- delete `slint`, `slint-build`, `raw-window-handle`, `anyhow(? still used)` from manifests; `build.rs` becomes no-op
- delete `slint-wgpu` feature, `egui-canvas`/obsolete features if any
- `.github/workflows/ci.yml` drops the placeholder's legacy path (AmeUI verified instead)
- Linux hook drops the legacy layer (AMEUI CHECK stays)
- README/docs/licensing/`THIRD_PARTY_NOTICES`/`Cargo.lock` references scrubbed
- `git grep -i slint` returns only history

## Execution notes

- Work is distributed as slices in `AMEUI_MIGRATION.md` (slice 55+).
- Every port must keep Linux CI (`AMEUI CHECK OK`) green.
- Do not touch `../AmeUI` (owner's tree) — request APIs instead.
- Feature parity beats cosmetic parity: a Slint-only screen blocks nothing as long as the AmeUI equivalent exists and is wired.
