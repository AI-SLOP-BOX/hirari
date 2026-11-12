# Slint deletion gate — complete

State as of this revision: **the Slint stack is gone from Hirari.** No code depends on it, no build secret, no feature flag.

## What happened

The desktop UI migrated slice-by-slice from Slint to AmeUI under the guidance of `docs/AMEUI_MIGRATION.md` (kept in the repository as history, but the migration list is complete). The plan note in that document said: "when the port reaches parity, make the AmeUI surface the default entry and delete the Slint stack." The plan completed over slices 55–66 and the final commit series that removes `hirari-ui/ui/**/*.slint`, `hirari-ui/src/ui/`, `hirari-ui/src/slint_ui.rs`, `hirari-ui/src/slint_ui_parts/`, and `hirari-ui/src/moufu.rs`, the `build.rs` Slint compiler, the `slint-wgpu` feature and its associated `pub mod slint_ui`/`pub mod ui` cfg-gating in `main.rs`, and the `slint`, `slint-build`, `raw-window-handle`, `wgpu`, `bytemuck`, `egui`, `egui-wgpu` optional dependencies.

The `ui_shared` module farm, which used to be `pub mod ui_shared` with `hirari-ui/src/ui_shared/*.rs`, is the neutral home of measure math, geometry, template starters, recent projects, render paths, `libui` step replacement, moufu helpers, SEARCH/FORMAT/STEM utilities, GPU plot rasterization, ploppable mouse routing, arrangement track parsing, and the render-ops/helper tree that the AmeUI shell shares with the (now-deleted) Slint surface.

The Rust worker exports a global-capability probe `build-tools/hirari-plugin-host-worker --capabilities` used by the headless flow to report `clap`, `au`, `vst3` support at startup.

## Verification

- `cargo check --workspace --locked` passes on the internal Linux CI (AMEUI CHECK OK) with the sibling AmeUI tree at the same commit the Linux builder fetches.
- `cargo test --workspace --locked` passes (`TEST OK` on the internal CI).
- GitHub Actions run `fmt`, `cargo check -p hirari-core-bridge`, `cargo check -p hirari-moufu-adapter`, `cargo check -p hirari-plugin-protocol` and the frozen-DSP-equivalence differential tests using a manifest-only placeholder for the private `ame-ui` path dependency so `cargo metadata` resolves without the private tree. The placeholder is never built; the desktop UI is never linked in that workflow.
- `git grep -i slint` returns nothing in source; the only remaining surfaces are (1) this document, (2) the migration history kept for reviewers, (3) `LICENSE-COMBINED-DISTRIBUTION.md` rewritten to reflect MIT-licensed AmeUI, and (4) historical archive docs dated before 2026-11.

## Distribution

Hirari's desktop bundle now ships AmeUI. The `scripts/build_app.sh` pipeline is unchanged; the artifact is byte-different but has no Slint obligations. `THIRD_PARTY_NOTICES.md` lists AmeUI as MIT and carries no `AboutSlint` or Slint attribution requirement. `README.md`, `docs/RELEASE_READINESS.md`, and `ARCHITECTURE_STATUS.md` reflect the same change.

## What did not happen

Some surfaces that were already absent from the Slint-era UI are intentionally absent here too, and others landed much later than the final deletion — please check the migration log instead of this top-level summary for the live status of every slice. As of this revision:

- **Sample editor waveform view** — done as an inline card embedded in the arrange dock (REVERSE/PITCH±SEMITONE/SPLIT/FADE IN/FADE OUT (normalized)/LOOP/DUPLICATE/DELETE wired to the engine). The deeper canvas-driven audio-editing surface with visible fade handles and warp anchors is unshipped.
- **Notation view** — only MusicXML export is available (P15, `2c9537e`). The staff-notation engine is not exposed; there is no attempt at rendering notation.
- **Flex pitch** — done (P8, `cbe3844`): PRESERVE PITCH (WSOLA) toggle, SPEED slider, PITCH slider, SYNC GROUP with apply, and QUANTIZE SYNC GROUP (strength/grid/swing) routed to the engine's `set_region_pitch_preserve_warp`, `set_region_warp_ratio`, `set_region_pitch_semitones`, `set_region_sync_group`, `quantize_region_sync_group`. The original knob-based surface is not present.
- **Comping take editor** — done (P10 partial, `ed64ac1`): the COMPING card lists the engine's takes from `comping_snapshot_json`, applies crossfade normalized through `set_comp_crossfade_normalized`, and promotes a take with `select_comp_take`. Comping swipe/rotate on the arrange canvas is not done; the canvas-internal workflow for promoting a span, as the original surface offered, is still absent.
- **Track stacks / global tracks** — done (P10, `c4be57d`): the TRACK STACKS card wires CREATE FROM SELECTED / ADD SELECTED / GAIN / COLLAPSE / DELETE to `upsert_track_stack`, `add_track_stack_member`, `set_track_stack_gain`, `set_track_stack_collapsed`, `delete_track_stack`. Markers on the global timeline are wired as a separate card. The global-track rack (mute/solo/pan across stacks) is not present.
- **Step sequencer / page editor** — done (engine ext): the bridge now stores the six lane-pitches, resolution and feel per track via `set_step_sequencer_patterns_json` (track-scoped, region_id == 0). The STEP SEQ card renders the 16-step page grid for the selected MIDI/Instrument track, RESOLUTION 1/4–1/64, FEEL Straight/Triplet/Dotted, lane −/+ pitch (unique) and ADD/REMOVE cells; notes flow through the normal `replace_midi_note_contracts` Undo transactions. Region-scoped patterns (region_id != 0) are also accepted for compatibility; playback still comes from scheduled notes.
- **MIDI Learn** — done (P11, slice 75): a MIDI LEARN card (ARM MACRO 1–8, INSERT RECENT MIDI) backed by CoreMIDI polling (`start_midi_input`, `poll_midi_input_json`, `handle_midi_cc_from_device`) copied from the old `ui/midi_input.rs` into `hirari-ui/src/ameui/midi_learn.rs`. Polling starts when the MIDI panel opens and stops when it closes; Note On/Off events fill a 30-second retrospective buffer; CC binds to the armed macro via `add_midi_learn_mapping`.
- **Sample editor pitch/give visual editor** — the pitch anchor drag surface from the Slint UI is unshipped; in its place, REVERSE/PITCH± are wired to the engine with a card caption.
- **Wave editor full** — the same as above; the dedicated WAVEFORM page with its handle-drag interaction is unshipped, but every engine-backed operation that the dedicated editor offered is reachable through inline cards on the arrange surface.
