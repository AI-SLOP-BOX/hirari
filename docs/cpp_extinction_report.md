# C++ Extinction Progress Report

Reproduce every number below with the commands in the appendix.
Baseline: `origin/main` (pre-migration merge-base). Current: `6e62715`.

## 1. Headline (all tracked C++/ObjC++/C)

| ext | baseline files | baseline LOC | HEAD files | HEAD LOC | delta LOC | reduction |
|-----|---------------:|-------------:|-----------:|---------:|------:|----------:|
| hpp  | 643 | 85922 | 252 | 56873 | -29049 | **33.8 %** |
| inc  |  68 | 21107 |  68 | 21309 |    +202 | -1.0 % |
| cpp  |  93 |  7951 |  89 |  7903 |     -48 |  0.6 % |
| h    |   6 |  4564 |   3 |  3234 |  -1330 | **29.1 %** |
| mm   |  11 |  1947 |   9 |  1538 |    -409 | **21.0 %** |
| **TOTAL** | **821** | **121491** | **421** | **90857** | **-30634** | **25.2 %** |

HEAD split: headers 60156 LOC, implementations 9339 LOC.

### Classification (vendor + generated reported separately, excluded from primary metric)

| class | kind | baseline files | baseline LOC | HEAD files | HEAD LOC |
|-------|------|---------------:|-------------:|-----------:|---------:|
| PROJECT | header | 703 | 81163 | 311 | 52299 |
| PROJECT | impl   | 103 |  9726 |  97 |  9269 |
| VENDOR  | header |   7 | 29381 |   5 | 28068 |
| GENERATED | header |  7 |  1049 |   7 |  1049 |
| GENERATED | impl   |  1 |   172 |   1 |   172 |

**Project-owned reduction: 90908 -> 61568 LOC, -29340 (-32.3 %).**
Vendor (-1313) and generated (unchanged) are reported separately and are not the goal.

### Compiled production vs dead/test/vendor

Liveness = transitive `#include` reachability from the real bridge translation units
(`hirari-core-bridge/src/hirari_core_ref.cpp`, `analysis_bridge_accessors.cpp`,
`hirari_core_audio_bridge.cpp`), not guessed.

| bucket | files | LOC | headers | impl |
|--------|------:|----:|--------:|-----:|
| **live / compiled** | 268 | 46129 | 44370 | 1759 |
| dead or unreferenced project files | 140 | 15439 | 7929 | 7510 |
| vendor | 5 | 28068 | 28068 | 0 |

`src/core` has only **10 `.cpp` files**: nearly all engine code lives in `.inc` files
`#include`d into TUs, so "implementation LOC" is overwhelmingly header code by construction.

## 2. Remaining C++ state owners

Delegation ratio (`hirari_*` call sites / LOC) separates genuine owners from facades.

| cluster | LOC | delegation | verdict |
|---------|----:|-----------:|---------|
| `src/core/rust_ffi.hpp` | 2768 | 48 % | FFI declaration surface, not state |
| `hirari_unified_engine_part_4a.inc` (region edit/analysis) | 1578 | **0.4 %** | **genuine C++ owner** |
| `engine/track_part_3a.inc` (region storage/mutators) | 946 | 1.8 % | **genuine C++ owner** |
| `hirari_unified_engine_part_1a.inc` | 669 | 2.7 % | mostly C++ owner |
| `engine/track_part_1a.inc` | 550 | 5.8 % | mostly C++ owner |
| `hirari_unified_engine_part_2a/3a/5a/6a/6b/7` | 2733 | ~1 % | **genuine C++ owner** |
| `audio_engine_public_part_1/2.inc` | 1350 | 1.3 % | **thin facade** over `HirariUnifiedEngine` |
| `engine/routing_engine.hpp` | 324 | 15.1 % | facade + C++ residual |
| `concurrency/audio_task_manager.hpp` | 272 | 8.1 % | facade + C++ residual |
| `driver/mac_audio_driver_host.mm` | 567 | 0.2 % | platform glue (stays C++) |
| `plugins/*` (clap/vst3/au hosts, sandboxes) | 3074 | 0-6 % | third-party ABI glue (stays C++) |

`AudioEngine` and `audio_engine_public_part_*.inc` are **facades, not state owners**.
Real state is `HirariUnifiedEngine::m_tracks` and `Track::m_regions` /
`Track::m_regionSnapshot`.

**Similarly named Rust types are not equivalents:** `RegionOrchestrator`
(`hirari-core-bridge/src/regions.rs`, 600 LOC) has no production caller outside its own
unit tests, and `EventProcessingHistory`
(`src/core/editing/event_processing_history.hpp`) is never instantiated. Neither counts as
"already migrated"; the live path is `Region::processingHistory`.



## 3. Three substantial migration candidates

### Candidate A - Region authoritative state (Region struct) -> Rust
- **Before:** `Track::m_regions` (`src/core/engine/track_part_3a.inc`) is the sole
  authoritative store, mutated by ~40 `set_region_*` methods in
  `hirari_unified_engine_part_4a.inc` (1578 LOC). Each repeats: validate -> lock
  `m_tracksMutex` -> linear-scan track -> mutate -> `publishRegionSnapshot()` ->
  `m_undoManager.recordAppliedAction`.
- **After:** Rust owns the region table; C++ keeps only a published immutable snapshot.
- **Production callers:** `command_executor.rs` (all `CommandAction::SetRegion*`),
  `hirari_core_project_loading.rs` (project load),
  `hirari_core_arrangement_midi_monitor.rs` (diagnostic JSON wrappers).
- **Dependencies:** undo manager, region snapshot publication, project serializer
  (`project_serializer_part_2.inc` / `part_4.inc`), every `part_*.inc` cluster.
- **Tests required:** differential encode/decode round-trip against the Rust codec in
  `project_region_decoder.rs` (already mirrors the same schema), plus per-field negative
  controls (NaN, out-of-range, missing track/region, 4096-entry caps).
- **Realtime safety:** all these run on the control thread; the audio thread only reads the
  immutable `m_regionSnapshot` pointer. The Rust move must preserve the same
  acquire/release publication contract and stay non-blocking on the audio thread.
- **Estimated removable:** 1500-2000 LOC (`part_4a.inc` + track mutators +
  `EventProcessingStep`/`AudioNoteSegment` storage) once Rust is authoritative.

### Candidate B - Audio-note (VariAudio) segment/curve cluster -> Rust
- **Before:** `AudioNoteSegment`/`AudioNoteAnchor` in `Region::audioNoteSegments`
  (162 LOC header `src/core/editing/audio_note_segment.hpp`), driven by ~11 engine methods
  in `part_4a.inc` lines 455-875 (~420 LOC) plus `track_part_3a.inc` storage and serializer
  views. All math is already Rust (`hirari_audio_note_curve_*`); C++ only stores and orders.
- **After:** Rust owns segments plus the derived `pitchRatioIntegralPrefix` and
  `pitchRatioCorrectionBeforeSeconds` projections (already Rust-computed).
- **Estimated removable:** 600-900 LOC of C++ storage/marshalling.

### Candidate C - Undo / transaction history -> Rust
- `m_undoManager` (`engine/undo_transaction_manager.hpp`) plus ~40
  `recordAppliedAction`/`performAction` call sites threaded through every region mutator.
  Rust already has `hirari_core_arrangement_history.rs`.
- **Estimated removable:** 800-1200 LOC. It is the shared dependency that makes Candidate A
  cheap rather than risky.

**Recommended order: A, executed incrementally, starting with the slice below.**
