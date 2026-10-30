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

- The engine status queue is now a process-wide Rust singleton with
  `pushEngineStatus`/`popEngineStatus` free functions, so the C++ `StatusQueue`
  class, its inline state storage and the handle-based queue ABI are gone.
- The realtime arena now hands out the scheduler's job storage through a
  process-wide Rust pool, and the engine's per-block scratch reset calls the
  Rust entry point directly; the C++ `RealtimeMemoryPool` and
  `ForensicScratchpad` adapters and their headers are gone.
- Plug-in scan admission is now path-based: `PluginCacheManager`'s key
  computation, content fingerprinting, blacklist record and persistence moved
  into the Rust scan cache, so callers pass only the plug-in path and the
  obsolete blacklist handle API (and its C++ declarations) is gone.
- Plugin preset/preset-automation, region comp-range and range-edit calls,
  spatial/HRTF placement, track and plugin automation curves, and the whole
  undo/redo/transaction surface now dispatch from Rust straight to their Rust
  implementations; the `AudioEngine` relays, their cxx declarations and the
  C++ declarations of those FFI entry points are gone.
- Track freeze/unfreeze/cache-path/restore calls and plugin state get/set now
  dispatch from Rust straight to their Rust implementations; the `AudioEngine`
  relay methods, their cxx declarations and the C++ declarations of the freeze
  and plugin-state FFI entry points are gone.
- Track volume/pan/mute/solo edits and undo clearing now go straight from
  Rust to their Rust implementations; the `AudioEngine` relay methods, their
  bridge declarations and the C++ declarations of those FFI entry points are
  gone. `RegionNoteEdits` also drops the note-curve mutators and pitch-integral
  prefix that only fed the deleted `AudioNoteSegment` helpers, and unused
  engine members (`m_vibeData`, `m_vibeVersion`, `m_workspace`,
  `m_pluginGuiStates`) are removed.
- Unreferenced engine helpers and types are deleted: the tempo-map
  signature/event readers, `PurityPassProcessor`, and the analysis-engine,
  PSOLA pitch-shifter and haptic-metadata headers no longer exist. The live
  `MIDINote` ABI struct now lives with its only consumer
  (`midi_region.hpp`) instead of the catch-all `audio_types.hpp` header.
- Dead bridge surface is removed rather than left as a second entry point:
  the borrowed track-peak accessors (Rust only ever used the owned copies),
  `initialize_gpu` (Rust calls the status-returning variant), and the
  `AudioEngine` forwarders for `start_audio_device`,
  `set_track_solo_for_offline_render`, `clear_preview_sample`,
  `get_track_count` and `get_region_audio_interleaved` are deleted together
  with the engine members only they reached: the arrangement marker/part
  accessors, tap-tempo helpers, automation-lane group helpers, and the
  telemetry getters. Marker and part storage still round-trips through the
  project file; the tap-tempo and automation-lane behaviour lives in Rust.
- The C++ `MidiLearnManager` and `PreviewSynth` wrappers are gone. The engine
  now holds the Rust-owned midi-learn and preview-synth handles directly, so
  CC binding, CC14 binding, preview reset and preview block rendering call the
  Rust implementation without an intermediate C++ object.
- Four C++ contract programs that no build script or CMake target referenced
  are deleted. Their contracts are covered elsewhere: the plugin-sandbox IPC
  assertions by the Rust `plugin_sandbox_workflow` integration tests, the
  MIDI-buffer ordering by the Rust differential reference test, and the VST3
  and input-queue checks asserted the fallback paths those types still keep.
- The harmonic-context singleton moved to Rust (`harmonic_context.rs`), so
  structure analysis and auto-arrangement read the same Rust-owned
  tension/valence/stability state. The unused C++ `TrackPresetStore`,
  `ChannelConfiguration` and `ChannelConfigurationStore` types are deleted;
  nothing constructed or referenced them.
- The unused C++ `MasteringKernel` adapter and twelve dead FFI declarations
  (bus-audio clear/commit/samples, automation-lane removal, VariAudio split,
  group warp maps) are deleted. The mastering behavior was already Rust-owned,
  and no C++ call site referenced any of these entry points.
- Stem separation now lives entirely in Rust. `stem_separator_registry.rs`
  owns the backend selection, model validation, status naming and split
  dispatch, and `stem_extraction_pipeline.rs` owns the background workers that
  feed it; the C++ provider, splitter wrapper and extraction pipeline, their
  declarations and the unbuilt C++ test are deleted, with the contract moved
  into Rust tests.
- The sidechain manager's gate, writer lock and control/reader entry points
  now live in `sidechain_manager.rs` behind one engine handle. Calling into a
  link is a single FFI call; the C++ class keeps only the transfer types and
  the capture callback that materializes `LinkState` values.
- The `ChordTrigger` processor wrapper is gone. It held a Rust handle and
  forwarded every call to `midi_fx.rs` with no production registration, so the
  class, its wrapper smoke test and the nine unreachable declarations it needed
  were removed.
- The MPE manager adapter is gone. `MPEManager` only wrapped
  `hirari_mpe_manager_*` and its sole caller used `reset()`, so the engine now
  holds the Rust handle and calls `hirari_mpe_manager_reset` on project reset;
  five unreachable declarations were dropped from the FFI header.
- Bus summing handles are no longer a C++ class. `Bus` forwarded every call to
  `bus_audio.rs`; `BusSystem` now creates the per-bus handles once, binds them
  into the Rust-owned system, and hands the raw handle to `BusTrack`, which
  calls `hirari_bus_audio_*` directly. Nine routing/audio declarations that no
  C++ code reaches were dropped from the FFI header.
- Routing state now has one owner. The graph, gain, feedback, send-PDC and
  topology stores live in `hirari-core-bridge/src/routing_engine_core.rs`
  behind a single opaque handle; the C++ `RoutingEngine` class, its 39
  forwarding methods and their direct FFI declarations are gone, and the
  engine calls `hirari_routing_engine_*` from the audio and control paths.
  Removed connection/feedback snapshots are returned in Rust-owned arrays
  with matching release entry points.
- Project-layout JSON serialization now runs in Rust. The engine gathers a
  borrowed model snapshot under the track lock and calls
  `hirari_project_layout_write_json`; key order, escaping, six-decimal number
  formatting, plug-in state padding/hashes, bootstrap-track handling, and
  cancellation all have one implementation in
  `hirari-core-bridge/src/project_layout.rs`. `get_project_generation` now
  fingerprints the Rust buffer instead of streaming through a C++ streambuf.
- MIDI expression-map edits now run through the Rust expression-map edit host.
  Rust admits the packed rows and JSON size, snapshots the current articulation
  map, skips an identical write, rewrites every articulation row and records
  one coalescing `Edit MIDI Expression Map ...` entry whose undo and redo
  replay the full snapshot by track id. The engine only resolves a track's
  articulation handle and stores the map JSON behind two host callbacks; the
  packed visitor/validator FFI and the track's articulation wrappers are gone.
- Scheduled MIDI note edits now run through the Rust note-snapshot store. Rust
  owns the 100,000-note admission, the read-modify-publish sequence under one
  edit mutex, the range edits and the legacy upsert, and records one
  `Replace MIDI Notes` history entry whose undo and redo replay the previous
  and new lists. The engine's publish lambda, control-side note mutex and the
  direct range/upsert FFI declarations are gone; the snapshot read is a
  count-then-copy call.
- Plugin sandbox maintenance now runs in Rust. Rust serializes the pass, keeps
  the observed health of every sandbox by engine address, decides the
  failure/recovery/quarantine/dropped-MIDI edges, formats the event labels and
  owns the packed status protocol the UI decodes. The engine only enumerates a
  track's statuses, restarts its rejected sandboxes and posts the event with
  the current playhead. The maintenance mutex, observed-health map, status
  vector accessor and its packing loop are gone.
- Plugin preset save and load now run through the Rust plugin-edit host. Rust
  checks the preset's plugin type against the target plugin, copies the current
  state for the undo entry, applies the loaded bytes and records one
  `Load Plugin Preset` action through the host; saving validates the path,
  plugin and non-empty state before calling the preset writer. C++ keeps the
  track lookup and state storage behind two host callbacks, and the engine
  methods are one-line delegations.
- Track freeze requests now run through the Rust freeze host. Rust owns the
  64 Mi-sample admission limit, validation order, project-end sizing (last
  region end versus two blocks) and absolute path resolution for disk freezes,
  and drains the audio callback only after a request is admissible. C++ keeps
  the track lookup, offline render, published snapshot and file writer behind
  host callbacks; the engine freeze methods are gone and the public layer calls
  Rust directly.
- Control-command admission and track spatial policy now run in Rust.
  `push_command` target lookup, project/audio generation rechecks, queue-full
  reporting, forensic-journal ordering and the last-command error are Rust-owned
  (the engine keeps the lock-free queue and its consumer); unreadable
  generations fail closed instead of matching generation zero. Spatial
  coordinates are rejected before lookup when non-finite, and HRTF kernels must
  be non-empty, stereo-matched and within 128 taps. The engine exposes
  track-queue/panner callbacks and the deleted engine methods are replaced by
  direct Rust calls from the public layer.
- Automation record-mode admission now runs in Rust. Wire-mode normalization
  (unknown values fail closed to Read), the translation onto the recorder's own
  mode vocabulary and the engine/recorder lockstep live in the recorder module;
  the engine keeps the atomic the audio callback reads. The bridge no longer
  clamps requests with `min(4)` before validation, so an invalid mode can no
  longer arm AutoPunch recording.
- The built-in effect catalog now lives in Rust. Serialized plugin types map to
  processor paths through one table (`Hirari/BusLimiter` in a bus's limiter
  slot), unknown types stay unresolved instead of loading an inert processor,
  and the name-to-type bridge lookup shares the same table. Track and bus
  chains ask Rust for the path; the C++ host still constructs the processor.
- Auto-arrangement now runs in Rust. The arrangement module gathers the region
  spans, sizes the project from its latest region end (saturating instead of
  overflowing), asks the structure analyzer for sections and emits one marker
  per section with its length. C++ keeps the region storage and the event queue
  behind the listing callbacks and the marker sink; an empty project still
  reports failure and a project without regions still emits nothing.
- Bounce and WAVE diagnostics are Rust-owned. The diagnostic bounce entry
  point and its JSON codes share one validator with the bool bounce API, so
  both accept exactly the same paths (an uppercase `.WAV` no longer fails only
  on the diagnostic path), and the WAVE loader's supported-format gate lives in
  the same place. The C++ bridge methods no longer carry a second copy of the
  path/format checks.
- Sandboxed-plugin slot bookkeeping now runs in Rust. Finding a plugin's sandbox
  slot, shifting the remaining slots down when a plugin is removed, and
  re-pointing plus re-sorting them when a plugin is moved share one module with
  tests, so the slot list can no longer drift from the effect-chain order; the
  engine keeps the slot and path storage and rebuilds the parallel path list
  from the returned positions.
- The packed MIDI note vector between the UI bridge and the note table is now
  decoded and validated in Rust. Track/region ids, pitch, velocity, channel,
  articulation, non-zero length, probability, the 100k note cap and the
  start/length overflow check all live in `midi_note_edit.rs` with tests, and a
  rejected vector leaves the caller's buffer untouched; packing the snapshot
  back into the nine-field vector is Rust-owned too.
- MIDI clock receive state now lives in Rust. The external-sync clock rate
  (validated as finite, above 1 BPM and below 1000 BPM), the tick counter and
  the last tick timestamp moved to `external_sync_runtime.rs`, which also keeps
  the wrapping tick increment and tolerates a null state. The C++ header that
  held the class, its unused MTC helpers and an unreferenced MMC enum is gone.
- Warp-marker replacement now converts the bridge's three parallel slices
  (source samples, timeline samples, transient flags) in Rust. Slice-length
  agreement and the 100k marker cap are checked before any conversion, so a
  rejected request allocates nothing; the converted list still runs through the
  shared normalizer before one marker-aware undo entry is recorded.
- Audio-device admission from the environment now lives in Rust. The
  `HIRARI_AUDIO_MODE=offline|none` gate and the `HIRARI_NATIVE_TEST_ISOLATION=1`
  flag are read in `audio_device_mode.rs` with tests, replacing five copies of
  the `getenv`/`strcmp` checks across engine construction, device restart,
  reconnect and configuration apply.
- MIDI note helpers on the realtime buffer are Rust-owned. Note on/off and
  All Notes Off validate the 1-based channel range and compose the status
  bytes in `midi_buffer_ops.rs`, so the C++ buffer only forwards arguments and
  an out-of-range channel can never produce an event.
- Audio configuration bounds now live in Rust. The tempo
  (finite, above 0 and at most 999 BPM), sample-rate (8 kHz to 384 kHz) and
  block-size (1 to 16384) checks in `audio_config_rules.rs` gate both the
  bridge's `apply_config`/`try_apply_config` and the engine's
  `prepareToPlay`, replacing three copies of the same predicate.
- Realtime bus stage calls now rely on Rust validation only. `addSamples`,
  `replacePostSamples`, `commitDryToPost` and `read` forward straight to
  `bus_audio.rs`, which already rejects null buffers, zero or oversized
  frames and a non-finite gain; `clear` clamps the frame count in Rust too, so
  the duplicated guards in the C++ bus header are gone. A new Rust test covers
  each rejection path.
- Routing lookups and fanout now trust Rust validation. The node-range checks
  in `RoutingEngine` for route/send gains, feedback gains, send-PDC delay
  registration and retire, and the pointer/frame checks around
  `processSendPdc` and both fanout calls are removed; the Rust tables reject
  out-of-range nodes and the fanout/PDC entries already refuse null buffers and
  zero frames. An FFI-level test pins the absent-edge reads.
- Native plug-in editor sessions now live in Rust. `native_editor_sessions.rs`
  owns the `(track, plug-in)` key packing and the session table, so
  open/close/adopt/forget/embedded on `NativeEditorHost` forward to Rust while
  C++ keeps only the platform callbacks and never holds a lock across one.
- WAVE64 loading now trusts the Rust decoder's metadata. `read_wave64_float32`
  already rejects files above 512 MiB, channel counts outside 1..=32, a
  non-32-bit sample format, a payload that is not a whole number of frames and
  any non-finite sample, so `WavLoader::loadWave64` no longer re-checks the
  returned channel/frame/size fields before copying them into planar vectors.
- Automation lane state now lives in Rust. `automation_lane_group.rs` owns the
  per-lane linked/protected/preview flags and the rule that a protected lane
  rejects previews, so the engine's four `*_automation_lane` entries forward to
  a Rust handle and the C++ `AutomationLaneGroup` header is gone.
- The plug-in cache path is resolved in Rust. `plugin_cache_path.rs` applies
  the `$HOME/.hirari` fallback with `/tmp/.hirari`, creates the directory and
  owns the cached `plugin_cache_v8.bin` string, and `cacheKey` forwards empty
  or oversized paths to `plugin_cache_key`, which already rejects them.
- Stem-separator validation messages live with the checks. The code-to-text
  table moved from `validateStemSeparatorModel` into
  `stem_separator_validation.rs`, so C++ only maps a non-zero code to the
  message Rust returns.
- Sandbox status text moved to Rust. `plugin_sandbox_status_text.rs` maps the
  failure and state-mailbox error codes to their hyphenated status strings, so
  `failureText` and `stateErrorText` no longer carry switch tables and a new
  code cannot drift between the native and Rust sides.
- Stem-separator backend names are also Rust-owned, so the C++ adapter no
  longer restates the Heuristic / ONNX Runtime / LibTorch / External Process
  display mapping.
- The sandbox transport retry policy now lives in Rust.
  `hirari_plugin_sandbox_transport_retry_delay` decides which failures are
  transient (spawn, handshake, early exit) and returns the 25 ms doubling
  backoff or 0 for terminal failures, so `addSandboxedPlugin` only starts,
  closes and sleeps instead of restating the failure set and shift.
- Worker command-line diagnostics moved to Rust.
  `hirari_plugin_worker_parse_error_text` turns the parser's `error_kind` into
  the logged message next to the parser itself, so the worker no longer keeps
  its own copy of the kind-to-text mapping.
- Bounce lifecycle admission now lives in Rust. `bounce_plan.rs` owns which
  published states are terminal, cancellable, pausable or resumable, so the
  engine's retry, cancel, pause and resume checks no longer restate the state
  sets that the render loop publishes.
- Plugin scan admission moved to Rust. `hirari_plugin_scan_rejection` performs
  the empty-path, missing-path, directory and regular-file checks (plus the
  single-inspect fallback) and returns the rejection text, so
  `PluginScanner::scanInSandbox` only records the returned reason.
- Track work-buffer spectrogram features moved to Rust.
  `hirari_track_analysis_spectrogram` splits the first channel into ranges and
  returns the clamped rms/peak mix, so `Track::AnalysisEngine` no longer keeps
  its own bin arithmetic and non-finite sanitising loop.
- Sandbox block failure classification moved to Rust.
  `hirari_plugin_sandbox_block_outcome` owns the consecutive-overrun threshold,
  the process-failed latch and the MIDI-invalidation failure set, so
  `ProcessSandboxProcessor::processBlock` only stores the returned flags.
- Built-in effect parameter names moved to Rust.
  `hirari_builtin_effect_parameter_name` holds the compressor, limiter, tube
  saturation, sub-bass and chord-trigger name tables, so each C++ facade only
  copies the returned string into its caller buffer.
- Preview voice macro mapping moved to Rust. `hirari_preview_synth_process`
  now takes the macro state and derives morph, detune, drive, cutoff,
  resonance and output gain from targets 0..5, so the engine no longer
  computes those curves and just forwards `m_macroControl`.
- Worker format resolution moved into the Rust argument parser.
  `hirari_plugin_worker_parse_args` now resolves the auto/explicit CLAP, VST3
  and AU adapter bits from the requested format and plugin path, so the worker
  reads one `resolvedFormats` word, and
  `hirari_plugin_worker_load_failure` maps a failed CLAP instance load to its
  sandbox error byte and exit status instead of the nested C++ ternaries.
- Audio-callback command sanitising moved to Rust.
  `hirari_engine_audio_command` clamps volume and pan, drops non-finite values
  and reports the setter to apply, so the engine's command loop only switches
  on the returned id instead of repeating the finite/clamp checks inline.
- Hardware-callback chunking moved to Rust. `hirari_engine_chunk_plan` owns the
  4096-frame engine cap, the split at the loop boundary and the in-place cycle
  restart, so `processBlockDirect` only forwards the current playhead and loop
  bounds and processes the returned chunk length.
- Recorded-automation draining moved to Rust.
  `hirari_track_automation_capture_drain` sizes and grows its own buffer and
  returns the pending points with a release entry point, so
  `Track::flushRecordedAutomation` no longer retries a caller-sized flush and
  only merges the returned points into the volume and pan lists.
- MIDI note snapshots moved to Rust-owned buffers.
  `hirari_midi_notes_snapshot_take` returns the published note list in storage
  it owns, so `midi_notes_snapshot` copies once and releases instead of
  growing a caller vector across repeated count/copy calls.
- Expression-map reads moved to Rust-owned buffers.
  `hirari_track_midi_expression_map_take` hands the stored bytes over with a
  release entry point, so `copyExpressionMapJson` no longer sizes a string
  from a count query and retries the copy until it stops growing.
- Track lifecycle edits now run through the Rust track-edit host. Rust owns the
  `Track <id>` / `Bus <id>` naming, the `Add Track`, `Duplicate Track` and
  `Remove Track` labels, and the undo/redo decisions: created and copied tracks
  are removed on undo and re-inserted at their captured index on redo, while a
  removed track travels with its sidechain links, sidechain routes and routing
  connections into a handle that restores them on undo. C++ keeps the track
  objects, deep clone, routing graph and the published audio snapshot behind
  host callbacks; the history releases each handle through Rust when the entry
  is dropped. Track insertion/removal now share one snapshot publisher and the
  plugin and track hosts share one replay recorder.
- Plugin chain add and remove now run through the Rust plugin-edit host. Rust
  checks the sandboxed bundle path, appends built-in or sandboxed plugins,
  captures the appended state and bypass (or the removed processor snapshot),
  and records `Add Plugin`, `Add Sandboxed Plugin` and `Remove Plugin` entries
  whose undo and redo replay through the host. C++ keeps only the track lookup,
  chain storage and snapshot lifetime; the history releases each edit through
  Rust when the entry is dropped, so removed processors are not leaked.
- Splitting and duplicating regions now run through the Rust region-edit host.
  Rust validates the split position, builds the split plan and the windowed
  note tables, applies both sides through payload ops, and records whole-region
  restore snapshots for undo; the duplicate policy and its undo capture moved
  with it. C++ keeps the payload and only clones/stores regions, and
  `split_region_with_right_id` / `duplicate_region` are one-line delegations.
  Splitting a region with an empty note table now succeeds instead of failing
  on the store splitter's empty-clear result.
- Replacing a region's audio now runs through the Rust region-edit host. Rust
  rejects an empty path, decodes the file, applies the 8 kHz..384 kHz rate
  policy, snapshots the region before and after the swap, and records one
  "Replace Region Audio" undo entry built from the two snapshots. The engine
  wrapper is gone and `Track::replaceRegionAudio` only stores the batch it is
  handed, so the swap policy has one owner.
- Comp-range batches now run through the Rust region-edit host. The packed
  words are checked and admitted by `hirari_region_comp_ranges_validate`, the
  accepted list and managed flag reach C++ as one payload, and the engine
  wrapper plus the duplicated Track admission code are gone. The setter never
  recorded undo, so the history is unchanged.
- Region range edits (gain-automation segments) now run through the Rust
  region-edit host. Rust validates upserts against the region length, applies
  the shared range-edit policy, stores the resulting list through a new
  range-edit payload, and records the undo pair; `set_region_range_edit` and
  `clear_region_range_edits` on the engine and Track were removed. C++ keeps
  the storage and now accepts Rust's sorted, bounded list verbatim.
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
