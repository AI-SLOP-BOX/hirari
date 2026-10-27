# Changelog

All notable user-visible changes will be recorded here. Hirari currently has no
stable release; entries describe the development preview and do not imply
production support.

## Unreleased

### Added

- Reproducible Rust toolchain selection and one-command developer setup.
- Strict dependency lockfile use in CI.
- Public contribution and security policies.
- Opt-in automatic rustup bootstrap via `HIRARI_AUTO_INSTALL_RUST=1`.
- AU/VST3/CLAP compatibility inventory reports with architecture and
  FOUND/SKIP status.
- Long-duration realtime callback soak and bounded external-plugin probes.
- Track rename/delete operations in the project model and CLI, with dependent
  regions and routes cleaned up transactionally.
- Waveform cache invalidation API that clears stale peaks across generations.
- Audio compatibility contracts for Unicode paths, malformed/empty files,
  mixed 44.1/48/96 kHz exports, and Missing-asset relinking.
- Explicit `transport_pause` command for clients that need pause semantics
  distinct from stop while retaining the current playhead.
- macOS GUI launch smoke test and a manual device/display E2E acceptance
  checklist.
- App-bundle release metadata packaging with SBOMs, portable ZIP output,
  SHA-256 generation, and tamper-detecting verification.
- macOS CI bundle job covering headless readiness, signature, and release
  metadata verification.
- Optional one-command installation of `cargo-audit` and `cargo-deny` via
  `HIRARI_AUTO_INSTALL_SECURITY=1`.
- Opt-in Moufu project-layout publishing, with filesystem paths and serialized
  plug-in state removed from shared snapshots.
- Bounded Moufu receive frames and queueing, with DNS re-resolution on retries.
- Moufu link and incoming-state notifications surfaced to the Hirari app.

### Fixed

- Starting playback on a Mac whose default output device has no capture side
  (for example the built-in speakers) now composes a private aggregate device
  with the default input instead of rejecting the device. The aggregate stays
  out of the device catalog, and the input endpoint reported to the UI and to
  recording remains the physical capture device.
- An explicitly selected audio device now negotiates its own sample rate and
  buffer size instead of inheriting the system default device's format.
- Quitting no longer hangs when the forensic journal writer is parked in its
  wake wait: shutdown now signals the writer after the stop flag is visible, so
  the journal thread always joins.
- Sample rates changed to glue an output-only device to the default input are
  restored when the private aggregate is torn down, instead of leaving the
  hardware at the project rate after quit.
- Changing the system default input now rebuilds a composed duplex aggregate
  around the new capture endpoint; a physical duplex device is left running.
- The device log now states that composing the aggregate keeps microphone
  capture enabled for the lifetime of the engine, instead of implying a silent
  fallback.
- Splitting an audio region with VariAudio note edits no longer shifts the
  right half's closing pitch/formant anchor one extra window to the left, so
  the edited curve stays continuous across the cut.
- A bounce whose plan is rejected (invalid range or sample rate) now resets the
  progress and publishes the failed state, instead of leaving the render
  status reporting "rendering" forever.
- The bounce writer no longer creates missing parent directories. A render to
  a path the caller never created now fails and reports the failure, as the
  writer's contract requires, instead of silently materialising the path.

### Changed

- Project-layout JSON serialization now runs in Rust. The engine gathers a
  borrowed model snapshot under the track lock and calls
  `hirari_project_layout_write_json`; key order, escaping, six-decimal number
  formatting, plug-in state padding/hashes, bootstrap-track handling, and
  cancellation all have one implementation in
  `hirari-core-bridge/src/project_layout.rs`. `get_project_generation` now
  fingerprints the Rust buffer instead of streaming through a C++ streambuf.
- Region trim and warp-ratio edits now run through the Rust region-edit host.
  Rust validates the request, computes the geometry, reads the existing warp
  markers, applies one geometry payload and records the undo pair; C++ only
  stores the fields and swaps the markers. The engine and Track wrappers were
  removed, so the geometry policy and undo capture have a single owner.
- Volume, pan, delay and plugin-parameter automation edits now run through the
  Rust automation-edit host. Rust validates the packed curve, applies the
  per-kind limits, snapshots the previous curve for undo and records one
  applied action; C++ only reads, stores and republishes the curve. The
  duplicated façade validators, the engine setters and the nested
  `AutomationPoint` type were removed.
- Track volume, pan, mute and solo edits now run through the Rust routing-edit
  host. Rust validates the range, reads the previous value, applies the
  resolved payload and records the undo entry; C++ only reads the control
  state and calls the track setters. The four engine setters and their
  declarations were removed, so the façade calls the same policy path as the
  routing and automation edits.
- Track naming, input-monitor focus and macro MIDI-learn bindings now run
  through the Rust control-edit host. Rust owns the admission rules (name
  length/NUL rejection, idempotent rename, monitor channel bounds and target
  existence, macro/controller/curve ranges) and the single `Rename Track`
  undo entry; C++ only reads the current name, applies the resolved mutation
  and updates the monitor atomics. The engine's duplicated validation and
  undo capture were removed.
- Plugin bypass, chain-reordering and parameter edits now run through the
  Rust plugin-edit host. Rust rejects a same-index move and non-finite
  parameter values before touching the chain, distinguishes a missing track
  from an invalid plugin index, reads the pre-edit value, applies the edit
  and records exactly one `Set Plugin Bypass` / `Move Plugin` /
  `Set Plugin Parameter` entry; C++ only reads or applies the mutation. The
  undo pair keeps the exact requested parameter value rather than a clamped
  read-back, as before. The engine's duplicated undo lambdas were removed,
  while the post-edit structural sync stays at the engine boundary.
- VCA group and sandboxed-plugin path admission now live with the state
  owners in Rust. `hirari_vca_add_group` rejects a zero group, non-finite
  gain and gain outside 0..=8 before touching the group table, and
  `hirari_plugin_sandbox_path_supported` owns the accepted bundle
  prefixes/suffixes (case-insensitive, byte-level so non-UTF-8 paths keep
  working). The engine wrappers only forward and resolve, and the
  duplicated validator code was removed.
- Last duplicated validators in front of Rust-owned state were removed. The
  render tail is clamped once in the bridge (`0..=60` s, non-finite to zero)
  and the engine only converts it to milliseconds; project scale and region
  processing-step admission are decided by the bridge and the Rust step store,
  so the engine and Track copies of those checks are gone.
- Undo, redo, history transactions and undo/redo depth now run through Rust
  entry points. The stacks were already Rust-owned; the engine wrappers that
  read the depth, applied the history and logged the top action were removed,
  and Rust now posts the same `UNDO | UNDONE` / `REDONE` / stack-empty lines,
  rolls back an unfinished transaction when a new one starts, and calls back
  into the engine only for the structural resync a replayed edit needs.
- Master-gain edits now run in Rust. The bridge owns the finite/`0..=2.0`
  admission, reads the current value, applies the change through the facade
  setter and records one `Master Gain` history entry only when the value
  changes; loading a project validates the stored gain before writing it. The
  engine setter keeps nothing but the atomic store the audio callback reads.
- Region warp-marker edits now run through the Rust region-edit host. Rust
  normalizes the caller's list through the shared marker transform, rejects
  any list that would lose a marker, applies one geometry payload and records
  a marker-aware undo entry. The engine setter and its declaration were
  removed; the façade passes markers to the FFI directly.
- Bounce render planning now runs in Rust. Output-path/format acceptance, the
  arrangement extent from region loop counts and MIDI note ends, the empty
  fallback, range validation, effect tail, limiter look-ahead and the initial
  playhead all come from `hirari-core-bridge/src/bounce_plan.rs`; the engine
  only gathers spans and maps the result code.
- Pre-v42 region loading now migrates source-rate units in Rust. Trim/length,
  fades, warp markers and persisted range edits are converted and overflow
  rejected by `hirari-core-bridge/src/region_hydration.rs`, which also filters
  invalid range edits; the loader only fills the request and applies results.
- Bounce output now uses one Rust stream handle for every format (PCM16,
  PCM24, float32 RIFF/RF64, WAVE64 float). The per-format C++ `WavWriter`
  stream classes and the unused `WavWriter` façade were deleted, and the
  native contract tests exercise the C ABI directly.
- Rust formatting is a blocking CI check.
- Native-engine workspace tests run serially to protect the process-wide FFI
  lifecycle.
- Real-device AU/CLAP probes time out after 90 seconds and preserve logs.

### Known limitations

- Hardware, third-party plugin, OpenUtau round-trip, signing, notarization, and
  long-duration evidence remains machine-specific.
- Windows and Linux are contract-check platforms, not production GUI releases.
