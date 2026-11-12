# Architecture Decision Record: "Music-SVG" proposal (friend's new standard)

**Status**: DECIDED — defer implementation until the 800-line split campaign lands.
**Date**: 2026-11-12
**Author**: maintainer (after code-grounded review)

A friend proposed a new standard for Hirari: a three-layer separation with
lock-free synchronization, a declarative "music SVG" (AI-editable text
format with opaque plugin binaries), unified `kurbo::BezPath` curves for
UI drawing and DSP evaluation, `arc-swap` hot graph swaps, and an ARA2 host
"provided by the existing implementation". This ADR records the verdict.

The proposal is directionally right. Almost all of it is, however, a
re-expression of what Hirari already implements, plus two genuinely new
ideas worth adopting. Adopting the whole thing would be a rewrite in search
of a reason.

## Verdict at a glance

| Proposal component | Status in Hirari (evidence) | Verdict |
| --- | --- | --- |
| `kurbo` everywhere, one path for UI + DSP | `vello::kurbo` already used for notation/geometry; automation already stores `AutomationPointContract { time, value, curve }` and evaluates `NativeAutomationPoint` sample-accurately (`realtime_automation.rs`) | **Adopt A (unify)** |
| "Music SVG" AI-editable text format | Native format is already text: pretty JSON + contracts, atomic save (`project_mutations::save_atomic` → `SovereignPersistence::save_document`), `diff_project_json` tooling, schema versioned | **Adopt B (schema addition only, keep JSON)** |
| `BuiltinDsp` / `ExternalPlugin` unified enum | `PluginInstanceContract` + `is_dawproject_plugin_device` + opaque checksummed base64 state; DawProject devices round-trip | **Adopt B as JSON `BuiltinDataflow` kind** |
| `arc-swap` for graph topology | `arc-swap` is already a dependency; tempo map, holographic panner config, and the effect-chain `RuntimeSnapshot` hot-swap machinery exist in `effect_chain.rs` / `effect_chain_host.rs` | **No rewrite; measure first** |
| SPSC rings / raw atomic params everywhere | `hot_swap_state.rs` (U8 phase machine + quiesce watchdog), native timed-parameter events, shared-memory plugin mailbox | **No blanket rewrite; keep, add where benchmarks demand** |
| KDL/XML as first-class format | Two working formats already exist (native JSON, DawProject XML via `roxmltree`); KDL adds a third reader/writer for free | **Reject as canonical; optional export only** |
| "ARA2 existing implementation fits perfectly" | `ara2_protocol.rs` is a host-side *contract* (Document/Sequence/AudioModification snapshots); no Celemony SDK binding exists (licensing), `region_audio.rs` has a range-read guard only | **Nothing to wire. Reject for now** |
| "Music SVG" as an SVG | Name is wrong: it is a private patch format; `kurbo` does not ship a one-line `BezPath::to_svg`, you hand-roll path serialization. It is JSON, not SVG, not a standard | **Reject the name/standard framing; keep textual patch idea** |
| Vello 60–120 fps, RT thread never blocks | Already the architecture: AmeUI + Vello canvas, RT FFI engine behind `AudioEngine`, paint/metria via canvases | **Already true** |

## What we adopt

### A. Unify the automation curve model (after the split campaign)

One representation, consumed by UI paint, RT evaluation, and save:

- Introduce a `Curve` enum in the project contract layer:
  `Linear` | `Bezier { control_points: […] }` | `Step` (existing per-point
  `curve: f32` is migrated/preserved as `Shortcut` for compatibility).
- UI: stroke the same curve through `vello::kurbo::Path` (notation-style).
- RT: evaluate the same cubic (`kurbo::CubicBez::eval`), keeping the current
  hint-cached scan in `realtime_automation.rs`. FFI stays `NativeAutomationPoint`.
- Save: same enum in `AutomationPointContract`; existing `.hirari` projects
  upgrade on load (defaults, no hard break).
- Acceptance: property tests assert identical evaluation between the UI path
  and the RT evaluator on noisy inputs; a load test proves a pre-split project
  opens byte-identical in automation.

### B. `BuiltinDataflow` devices in the JSON schema (after A)

- Extend `PluginInstanceContract` with `kind: BuiltinDataflow { units, links }`
  describing a small explicit node graph (unit kind, normalized params, links).
- External plugins keep their opaque, checksummed base64 `state`. AI agents
  edit only `BuiltinDataflow` payloads and `region`/`midi` notes — the
  external-plugin boundary stays opaque and crash-safe.
- DawProject export embeds the same payload as a custom device kind or keeps
  the existing `plugin_devices_xml` behavior with an extra comment; DawProject
  is the interchange gate either way.
- Acceptance: round-trip through save/load and through DawProject XML proves
  plugin binaries are untouched by `BuiltinDataflow` edits.

## Explicitly rejected

1. **XML/KDL as canonical format.** JSON with contracts + `diff_project_json`
   works, has atomic save, and has schema discipline; a third format is
   fragmentation for aesthetics. (A KDL *export* is fine if someone wants it.)
2. **Calling it "Music SVG" or a standard.** It is a private textual patch
   format; branding it as a standard creates expectation debt and
   misdirection (no SVG involved).
3. **`arc-swap`/SPSC rewrite of the RT graph.** The design already exists in
   situ (process sandbox + effect-chain snapshots + atomic watchdogs). Blind
   rewrites of the RT path are the worst kind of architecture astronautics.
4. **Linking ARA2 now.** No Celemony SDK is bound; `ara2_protocol.rs` is a
   contract layer. Any "ARA2 integration" work before SDK licensing is fiction.

## Process notes and constraints

- **Disk space**: `/tmp/cargo-mac-ephemeral-target` is ~17 GB on a 228 GB
  volume; local `cargo test` fails with `No space left on device`. Linux CI
  remains the verification authority (`cargo check --workspace --locked`,
  `cargo test --workspace --locked`). Local workloads must step around this,
  or the cache gets cleaned by the owner.
- **`ame-ui` is a path dependency** (`../AmeUI`). Its working tree is dirty
  locally (macOS-only `block2`/`objc2-app-kit` additions). Committing a
  `Cargo.lock` generated against that dirty state breaks `--locked` on a
  clean checkout. The Cargo.lock must be produced against or reconciled with
  the clean AmeUI used by CI (see the Cargo.lock revert shipped in 7f18e19
  after CI failure at b0fa85b).
- **Vello as the single GPU pipeline**: every Hirari canvas paints into
  Vello; automation must not grow a second pipeline.

## Sequencing

1. 800-line campaign finishes (in progress; largest live file now
   `hirari-core-bridge/src/regions/region_edit_host.rs` ~4648 lines).
2. Phase A spike: one `Curve` enum + shared evaluator + tests. Measure, then
   migrate `AutomationPointContract`.
3. Phase B: schema addition behind the same contract discipline as the rest
   of persistence; AI boundary documented in the schemas (` BuiltinDataflow`
   is AI-editable, `ExternalPlugin` never is).
4. Re-evaluate ARA2 only after SDK licensing exists on this machine.

Keep this document in `docs/architecture/`. Do not start A or B opportunistically
in the middle of the split; they are both too wide for review riding alongside it.
