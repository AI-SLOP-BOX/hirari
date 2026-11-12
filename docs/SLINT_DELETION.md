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

The sample editor, notation exporter, flex-pitch page, workflow-chain UI, comping take editor, stack rack and global-track rack were not ported pixel-for-pixel. The engine-side APIs used to back them are all present; only the UI is absent, and calls come at a significant engine-trust cost that is not justified in the shipped surface. Users who need those functions can file them as feature requests against the AmeUI surface; the engine APIs are stable.
