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

Ordered by "engine-backed first" (usable as soon as they land in AmeUI).
Each item is a recorded slice in `AMEUI_MIGRATION.md`. **Status as of the
current head** (`d2e4d9a`):

| # | Surface | Status |
|---|---|---|
| P1 | Export dialog (render/MIDI/stems/cancel/MusicXML) | **done** |
| P2 | Diagnostics / health dashboard | **done** (`281c610`) |
| P3 | Full mixer / smart controls | **done** (via arrange strips + plugin strip) |
| P4 | Routing / aux sends | **done** (`160fee4`) — sends card |
| P5 | Moufu link page | **done** (`8a9b06f`) |
| P6 | Preferences / setup | **done** (`af60aab`) — focus mode |
| P7 | Mastering page (LUFS display; no static module tiles) | **done** (`608e9e9`) |
| P8–P12 | Workflow pages (flex, comps, stacks, step seq, MIDI learn) | **open** |
| P13 | OpenUtau editor | **partial** (`337f762`) — open/launch/import works; tuning worker not ported |
| P14 | Sample editor | **partial** (`d2e4d9a`) — REVERSE/PITCH±SEMITONE/SPLIT; waveform canvas edit suite open |
| P15 | Notation / MusicXML | **done** (`2c9537e`) — export-only |
| P16 | Synth panel | **done** (`15ac939`) |
| P17 | Video monitor | **done** (`1bd92d1`) — upload path works |
| P18 | Advanced automation | **wont port** — dead code, never instantiated |
| P19 | Global tempo editor | **partial** (`5153234`) — event list + tap tempo; no knob/curve canvas |
| P20 | Spotlight command palette | **done** (`6f56a51`) |
| P21 | Workflow pages remainder | **open** |
| P22 | Aux/VST transport collapse on rack focus | **open** |

**Net**: engine-backed core (render, diagnostics, sends, moufu, master
loudness, tempo, spotlight, synth, openutau, audio edit) is fully in AmeUI.
Remaining user-visible surface to port: notation view (not export),
synth panel deep voice-match editor, sample editor waveform suite,
and the workflow pages (flex pitch/comping/stacks/step sequencer/MIDI
learn/global tracks) with their shared stack-strip UIs. Those are the
P17/P18-sized slices.

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
