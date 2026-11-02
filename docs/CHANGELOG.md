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

- Track and region id allocation now lives in Rust: one allocator keeps the
  counters, the wrap-around walk that never hands out the invalid sentinels, the
  restart a new project takes and the reseed a loaded document gets. The engine's
  create, insert, clone and duplicate callbacks receive the id they must use, so
  they answer for storage only and no longer carry a counter or scan the project
  for a free number.
- Track and region id resolution now has one implementation shared by every
  engine host callback, and Track's edit protocol (drop the frozen render,
  mutate, bump the layout revision) is a helper instead of three statements
  repeated at each site; the messages for an unresolved track or region id are
  named in Rust, so the C++ surface only answers for ids it actually found.
- Sandboxed plug-in lifecycle state now lives in the Rust sandbox host: the
  prepared device format, the late-block streak, the recovery latch, the
  quarantine clear and the parameter replay cache are decided there, so the
  C++ processor only forwards blocks and lifecycle requests.
- Project-layout gathering now lives in Rust: the engine exposes its live
  tracks through an accessor table and `layout_gather.rs` owns the snapshot,
  its backing string/automation/plug-in blocks, region rate clamping and
  missing-file detection, then serializes or saves in the same call. The C++
  snapshot structs and the gather loop are gone from the engine.
- MIDI playback reconciliation now lives in Rust: the per-engine clock tracks
  the previous block end, published snapshot revision, playback pass and the
  transport-stop panic request, decides when voices must be reconciled, and
  drives the note scheduler, so the C++ callback keeps only the voice reset
  and note dispatch callbacks. The five continuity members and the panic
  atomic are gone from the engine.
- Dropped the C++ declarations for FFI entry points nothing on the native side
  calls any more (scheduled-note pack/unpack/upsert, preview engine binding,
  routing connection JSON, metronome enable, MIDI-learn CC and plugin/control
  edit relays).
- Transport tempo, MIDI clock rate/tick, playback state, sample rate and region
  warp-marker writes now run as bridge-side Rust entry points on the engine
  handle; the C++ `AudioEngine` relays, the engine const-qualification they
  needed, and the now-unused warp-marker FFI declaration are gone.
- Sandbox status enumeration, WAV diagnostic text, and waveform result polling
  now call the Rust routines directly from the bridge; the C++ `AudioEngine`
  relays, the local sandbox-status shim declaration, and the unused FFI
  declarations they needed are gone.
- New-project reset, watchdog counters, sandbox retry/restart, plugin bypass,
  native-editor availability, bounce controls, vocal-remover, articulation,
  track EQ, auto-arrangement, project scale, region-processing and automation
  record mode, video-frame requests and VCA assignment now run as Rust
  extension methods on the engine bridge; their C++ `AudioEngine` relays are
  gone.
- Loop/cycle/playhead and test-tone transport writes, master-gain writes with
  the project-state bump, and the offline-render target/tail/option setters now
  run as Rust extension methods on the engine bridge too, so the remaining
  `AudioEngine` relays and the C++ `noexcept` qualifiers that blocked the
  matching member pointers are gone.
- Building a track's published render inputs moved to Rust: the overlap
  crossfades, the spectral-stretch decision and the per-region field mapping
  are derived from a trimmed source struct, so the C++ publication path no
  longer carries the `Region` crossfade/spectral members, the overlap-view
  FFI declarations or the 39-field assignment block.
- Removed the unused Rust region-table mirror and its differential check: the
  published render inputs already carry the whole region state the audio
  callback reads, so the per-publish mirror copy, its FFI surface and the test
  that only compared the mirror with the same list are gone.

- Low-latency mode, preview triggering, metronome control, tempo reads,
  sample/beat conversion and the routing snapshot now run as Rust extension
  methods on the engine bridge, reaching the PDC, preview sampler, metronome,
  tempo map and routing objects through handle accessors; the matching C++
  engine relays and `AudioEngine` forwarders are gone.
- Track input monitoring, plugin reordering, MIDI CC learn dispatch, the
  preview-synth selector and sidechain links now reach the Rust policy layer
  directly, so the C++ engine relays and their `AudioEngine` forwarders are
  gone; the project-load sidechain pass calls the Rust routing edit itself.
- MIDI-note editing now runs as a Rust extension trait on the engine bridge:
  clearing, replacing, snapshotting and the range remove/transpose/move edits
  repack and call the Rust note store directly, so the C++ engine methods and
  their `AudioEngine` forwarding wrappers are gone.
- The spectral-stretch decision now receives a region's raw sample rates and
  warp ratio and derives the source-per-timeline mapping in Rust, so the
  duplicated C++ ratio helper is gone.
- Replacing a region's audio now resolves its timeline geometry in Rust: the
  decoded length and the source/timeline frame mapping produce the stored
  state, so the C++ setter only installs the handle it is handed.
- Insertion-time region validation now operates on the shared
  `HirariRegionState` record, so the C++ metadata transfer helpers and the
  separate add-metadata struct are gone and the fade clamp runs in Rust.
- Project loading now installs the region's editable state as returned by the
  Rust hydration pass (`hirari_region_hydrate_decoded`), so the persisted
  field mapping lives beside the state layout instead of being re-assigned
  field by field in C++; five C++-unused AudioBuffer declarations were removed.
- `AudioBuffer` dimensions, the channel view and externally wrapped planes are
  now owned by the Rust storage (`hirari_audio_buffer_storage_*`); the C++
  class keeps only one opaque handle and its pointer/resize/clear/gain
  operations. A dead write-only dirty flag was removed with the fields it
  guarded.
- Track freeze publication is now Rust-owned (`track_frozen_store.rs`): the
  published frozen buffer, its retire/clear lifecycle, generation counters,
  cache path and validation live behind `hirari_track_frozen_store_*`, so the
  C++ `Track` members for the frozen buffer, edit mutex and counters are
  deleted. Dead `IProcessor::ParameterDescriptor` /
  `getParameterDescriptor` API is removed with it.
- The audio input block queue is now used through the Rust SPSC
  implementation directly (`hirari_audio_input_queue_*`); the C++
  `MacAudioInputBlockQueue` wrapper class is deleted and only its capacity
  constants plus the plain `AudioInputBlockInfo` descriptor remain.
- Plugin identity now travels with the Rust-owned effect-chain entry
  (`kind` on `HostNode`, `hirari_effect_chain_node_kind` /
  `hirari_effect_chain_set_node_kind`), so the deleted C++ `m_pluginTypes`
  vector can no longer drift from chain order during add, remove, restore or
  move. Unreachable C++ declarations for retired range-edit, warp-marker,
  legacy-migration and pitch-analysis entry points are removed with it.
- A region's range-edit, comp-range and warp-marker tables are now Rust-owned
  (`region_clip_edits.rs`), and the `Region` vectors plus `compManaged` field
  are deleted. Each region holds an opaque store handle and a frozen projection
  published with the region snapshot, so the audio callback and the layout
  serializer read the tables lock-free through `hirari_region_clip_edits_*`.
  Region-add, the comp/marker/range setters and project hydration now hand the
  data straight to Rust instead of copying it into the region.
- The engine command and event queues are now Rust-owned single-producer
  rings (`engine_rings.rs`), with the C++ `RingBuffer` template and both
  engine members deleted. Each engine holds an opaque handle created and
  released by Rust, and every C++ command, event push and project-boundary
  clear goes through the ring ABI (`hirari_engine_command_ring_*`,
  `hirari_engine_event_ring_*`), so the queue storage and its
  acquire/release index protocol exist in Rust only.
- The diagnostic ring no longer carries a C++ owner: `log_buffer.hpp` and its
  `LogBuffer` wrapper are deleted, and every C++ site posts through
  `hirari_log_post`, which hands the message straight to the Rust ring.
  Engine log-state accessors and the CoreAudio log trampoline use
  `hirari_log_state`, so the bounded ring, its capacity, and its pop contract
  stay in Rust only.
  span and buffer admission, geometry staleness check and note-table commit
  live in the regional pitch host, which reads the region and its decoded
  audio through the existing region host callbacks. `Track` keeps only the
  locked commit and republish, `regionHostPitchAnalyze` and the unused
  `editing/audio_pitch_analysis.hpp` config helper are deleted, and the
  analysis handle travels as a `kHirariRegionEditPitchCommit` payload.
- The sandboxed plug-in worker's process lifetime now belongs to the Rust
  plug-in runtime: argument parsing, launch limits, mailbox mapping, protocol
  handshake, adapter selection, CLAP/AU/VST3 backend dispatch, and teardown
  run in `hirari-plugin-protocol`, whose `hirari_plugin_worker_main` is the
  entry point. The C++ `plugin_sandbox_worker.cpp` is reduced to the shared
  mailbox view accessors, the VST3 SDK hooks, and `main`, and the
  `plugin_sandbox_worker_main_start/backend/main_loop` include files are
  deleted.
- Project hydration policy now lives in the Rust bridge: plug-in type ids are
  decoded from the container codec, region paths/defaults are resolved on load,
  decoded regions are clamped and legacy pre-v42 geometry is migrated in Rust,
  and persisted VariAudio notes load through the Rust store. The C++ loader
  keeps only the audio decode and direct field copies, and the now-unused
  `hirari_region_vari_audio_store_load` C declaration and
  `Track::loadNoteSegments` wrapper are deleted.
- AudioUnit v2 hosting for the sandboxed plug-in worker now lives in the Rust
  plug-in runtime: bundle discovery, component and stream-format negotiation,
  render/MIDI/parameter callbacks, and the ClassInfo state envelope. The C++
  `au_sandbox_adapter.hpp` and the worker's AU backend branches are deleted, so
  AU, CLAP, and VST3 plug-ins share one isolated worker protocol.
- Write-only C++ diagnostic state and unused accessors are removed:
  `BlackBoxRegister`, the graph-dirty and articulation-hash writes,
  track/effect-chain tail sampling, `VideoSystem::update/duration`, the
  unread mac input-capture sink, and empty/zero-magnitude buffer queries.
  Command rejection codes are now read through the Rust admission journal
  (`engine_last_command_error`) instead of a C++ `AudioEngine` forwarder, so
  the same per-engine error table serves both native callers and the bridge
  tests.
- The C++ fragment-reassembler handle is deleted. `MidiFragmentReassembler`
  and `MidiBuffer::addFragment` were a wrapper around the Rust reassembler
  with no production caller; the Rust implementation keeps its own unit
  coverage, and the compile contract now exercises the shared-memory
  `MidiExtendedMessageRing` directly, which is the part the sandbox protocol
  still uses. The now-unused FFI declarations went with it.
- The abandoned Metal spatial kernel is deleted. Its only entry point,
  `initialize_gpu_with_status()`, built a device/queue/pipeline that nothing
  ever submitted work to: the async `processAsync` path had no caller, so the
  engine logged a GPU-fallback warning for a fallback that was the only path.
  `metal_audio_kernel.{hpp,mm}`, the orphaned `spatial_kernels.metal` shader,
  the cxx probe, its `HirariCore::gpu_initialized` field and the Metal
  framework link are gone; startup no longer probes a dead GPU pipeline.
- The C++ task-scheduler/thread-pool adapters are deleted. The Rust-owned
  worker queues were already the only implementation, and the C++ wrappers
  had no production caller left: the engine's private scheduler was started
  and stopped without ever accepting work. `thread_pool.hpp` and the
  `AudioTask`/`AudioTaskJob`/`AudioTaskStealingScheduler` types go, leaving
  `audio_task_manager.hpp` as the denormal guard it still hosts. The wrapper
  smoke tests are replaced by a native Rust `parallel_for` test that checks
  every index runs exactly once, and the compile contract drops its pool probe.
- Sandbox recovery orchestration now runs in Rust
  (`sandbox_recovery_runtime.rs`): the periodic maintenance loop, its
  opt-out flag, the forensic-journal start and the recovery log line moved
  out of the engine, so the C++ thread, condition variable, wake mutex and
  the maintenance forwarder are gone. The engine hands over its host pointer
  and journal handle once; shutdown joins the loop through the same handle.
- Dead C++ engine state that only wrote itself is removed: the "adaptive CPU
  governor" loops and its eco-mode flag/log, the always-zero spatial mode word
  passed to the output stage, the CPU-load meter (its only reader was the
  governor), and the unused ascend/video-sync flags with their log branches.
  Behaviour is unchanged; the audio callback no longer samples the OS clock.
- The dead C++ MIDI-region surface is gone: composition MIDI has been
  Rust-owned for a while, so `Track`'s region list was never populated and only
  the duplicate-track path walked it. `midi_region.hpp`, the `Track` add/get
  accessors, the duplicate clone loop, and the orphaned note-store C
  declarations are removed; the wrapper smoke test is ported to the Rust
  note-store ABI.
- Sandboxed-plugin slot bookkeeping now lives in a Rust-owned registry
  (`plugin_sandbox_map.rs`): a track's chain index and plugin path are stored
  as one slot list, and add/duplicate/remove/restore/reorder all go through it,
  so the two C++ vectors that had to be permuted in lockstep are gone. The
  dead per-processor generation/quarantine wrappers and the native-processor
  C ABI declarations they left behind are removed as well.
- The C++ `PluginSandboxHost` forwarding facade is gone: the sandboxed plugin
  processor holds the Rust-owned `SandboxHost` box directly and marshals
  audio/MIDI, so nothing outside Rust renames or relays the lifecycle, state
  and telemetry calls it already owned. The native contract drives the Rust
  host handle itself, and the realtime guard now watches the processor's
  block-copy boundary.
- Track automation list storage now lives in Rust
  (`track_automation_store.rs`): volume, pan and track-delay lists, plugin
  automation lanes and the captured-take merge are published as immutable
  Rust generations, the recorder drain appends and sorts them, plugin-lane
  replacement owns the erase-then-append merge, and the evaluator hint slots
  are Rust memory. C++ keeps only the opaque handle and its freeze/layout
  invalidation calls.
- Built-in plugin hosting is Rust-owned (`native_plugin_host.rs`): the
  128-slot parameter mirror, the dirty-bitmask block-boundary flush, kind state
  serialization/restoration with mirror resync, and the null-safe inert load
  path all live in Rust. `NativeProcessor` and the two `plugin_host_part_*.inc`
  files are gone; C++ keeps a thin `ExternalPluginHost` adapter that forwards
  the `DSP::IProcessor` virtuals, and the host contracts run as Rust tests.
- Effect-chain ownership, ordering and control-side mutation now live in Rust
  (`effect_chain_host.rs`): entry order, bypass/parallel state, snapshot
  publication, latency/diagnostic aggregation, legacy state markers,
  sample-accurate automation dispatch and reader admission are Rust-owned.
  C++ keeps only the `DSP::IProcessor` instances, the parallel scratch buffer
  and the sidechain view; the watchdog contract moved to Rust tests.
- Dead C++ surfaces that only duplicated Rust ownership are gone:
  `AudioEngine::shutdown_process` and `HirariUnifiedEngine::getInstance` (the
  engine is owned per bridge handle), plus 26 `rust_ffi.hpp` declarations of
  Rust exports that no C++ translation unit calls any more, including the
  CoreMIDI block the MIDI monitor bypasses.
- The silent fallback audio-device contract now lives in Rust
  (`fallback_audio_device.rs`). The C++ `Hirari::Platform` device abstraction
  had no production caller — the live engine opens CoreAudio through
  `mac_audio_driver` — so the header, its dedicated transition contract, and
  the fallback section of the realtime-safety contract are gone; the
  transition/generation matrix runs as Rust unit tests through the same
  CMake test target.
- CoreMIDI device access no longer crosses the C++ engine wrapper. The MIDI
  monitor calls the Rust CoreMIDI host directly for device listing, packet
  send, input start/stop/poll and host-time conversion, so the eight
  `AudioEngine` pass-through methods and their platform `#if` branches are
  deleted; non-Apple builds keep the same empty/false defaults in Rust.
- Region undo capture now runs in Rust. The region edit host copies warp-marker
  and range-edit payloads, adopts the note-table and region-snapshot handles,
  and commits one entry into the engine's Rust history, so the C++
  `OwnedRegionEdit` registry and `regionHostRecordUndo` callback are deleted.
  Region-alignment and quantize jobs gate on the same history availability.
- Plugin edit undo capture now runs in Rust. Bypass, move, parameter and preset
  loads record their undo/redo closures against the engine's Rust history
  through a new `undoState` host callback, so the four C++ undo-capture lambdas
  are deleted; the state-apply callback only stores plugin state.
- Automation curve edits now record their undo entry in Rust. The automation
  edit host copies both point buffers and commits the entry into the engine's
  Rust history through the shared `undoState` callback, deleting the C++
  `automationHostRecordUndo` capture.
- Track rename undo entries are recorded in Rust. The control edit host keeps
  the previous and requested names, replays the apply callback for undo/redo,
  and commits one entry into the engine's Rust history; the C++ `Rename Track`
  capture lambda is deleted.
- Routing undo entries are recorded in Rust. The routing edit host copies both
  payload lists and commits them into the engine's Rust history, replaying
  through the same payload applier with the requested coalesce/perform flags;
  the C++ capture lambda is deleted.
- Track and plugin lifecycle undo entries are recorded in Rust. Both edit hosts
  own the lifecycle entry through a shared handle that releases it when the
  history drops the entry, so the C++ `recordNativeReplay` adapter and its
  declaration are deleted.
- The C++ undo transaction façade is deleted. The engine now keeps the Rust
  history handle directly and clears it through the FFI, and the native
  compile contract drives the transaction API without a C++ wrapper.
- The `RegionNoteEdits` and `RegionProcessingHistory` C++ adapters are
  deleted. `Region` holds the Rust note-edit store, its published frozen
  projection and the processing-history store directly, and the engine,
  audio-analysis and project-load paths call
  `hirari_region_vari_audio_*` / `hirari_region_processing_history_*` itself.
- The `HolographicPanner` C++ wrapper is deleted. `Track` holds the Rust
  spatial-processor handle directly and calls
  `hirari_track_holographic_panner_*` for kernel, rate and state queries.
- The `MasterMeter`, `Goniometer`, `MasterLimiter` and `ChannelStrip` C++
  DSP header façades are deleted. The engine and tracks hold the Rust
  `hirari_master_meter_*`, `hirari_master_limiter_*` and
  `hirari_channel_strip_*` handles directly.
- The `AudioDecoderManager` singleton adapter is deleted. The engine's import
  path is now a private `importAudioFile` helper that calls the Rust decode and
  import-cache entry points directly.
- The `NativeEditorHost` C++ wrapper is deleted. `AudioEngine` owns the Rust
  native-editor session handle directly and calls
  `hirari_native_editor_sessions_*` for adopt, forget, lookup and embedded
  queries, dropping the unused platform-callback binding path.
- The `RegionTimeStretch`, `ProjectDB` and `WavLoader` C++ adapter headers are
  deleted. Regions hold the Rust stretcher handle directly with a destroy
  deleter, and the bridge calls the Rust `hirari_region_stretch_*`,
  `hirari_project_db_*` and `hirari_wav_diagnostic_json_*` entry points at the
  engine and contract-test call sites.

- The `MacMidiDeviceHost` and `PluginScanCache` C++ header adapters are
  deleted. The bridge now calls the Rust `hirari_core_midi_*` CoreMIDI and
  `hirari_plugin_scan_cache_*` entry points directly from the engine header,
  so MIDI device enumeration/input and plug-in scan-cache recording have a
  single, Rust-owned implementation path.

- The `ForensicJournaler` and `PluginCompatibilityRegistry` C++ adapter
  classes are deleted; the engine now calls the Rust `hirari_forensic_journaler_*`
  ABI directly for journal lifecycle and command logging. The unused
  registration snapshot accessor and its `hirari_plugin_registry_*` externs,
  which only ever returned an empty `[]` snapshot, are removed along with the
  dead recovery callback typedef.
- The unused `Core::GPU::MetalAudioKernel` duplicate is deleted. The only
  live Metal spatial kernel is `DSP::Spatial::MetalAudioKernel`, which the
  bridge still initializes and processes through; the second declaration and
  its bridge translation unit had no caller.
- Unreferenced C++ helpers are gone: the engine's unused `renderBlock`
  convenience overload and its musical-time structs, the worker VST3
  runtime's editor and unload entry points, the limiter's direct
  threshold/ceiling/release/lookahead setters (parameters already flow
  through `setParameter`), plus dead scheduler, MIDI-buffer and processor
  accessors.
- The master meter no longer recomputes an unused 64-band spectrogram on
  every UI read, and C++ no longer declares the Rust-internal goniometer and
  undo helper FFI that only Rust calls, nor the unused scheduler task
  posting and sandbox failure accessor wrappers.
- The in-process VST3 host scaffold is gone. VST3/AU/CLAP binaries are
  loaded only by the sandbox worker, so `VST3HostProcessor` had no caller
  left in the engine and its contract test now covers the native catalog and
  sandbox host instead.
- The built-in processor host now carries only the path it can actually
  serve. External VST3/AU/CLAP binaries already go straight to the process
  sandbox, so the unreachable in-host VST3 branch, sandbox ownership,
  health polling, native-editor forwarding and failure atomics were removed
  from `ExternalPluginHost`, along with a dead tempo-snapshot mirror in the
  FFI header.
- Undo commit and traversal now run in Rust. Registering both sides of an
  edit, replaying the redo on perform, recording with coalescing, discarding a
  failed pair, and the `UNDO | PERFORMED/RECORDED/UNDONE/REDONE` diagnostics
  line all happen in `undo_transaction.rs`, so the native manager only keeps
  the `std::function` closures it owns. Dead realtime-guard helpers and
  unreferenced FFI type aliases were removed with the same pass.
- Media import caching and freeze-cache restore moved to Rust. Path and file
  identity checks, hit/miss, eviction, and dead-buffer purge now run in
  `audio_import_cache.rs` behind opaque weak handles, and freeze restore
  decodes, enforces the rate/length contract, and widens mono artifacts in a
  Rust reader instead of the native `WavDecoder` facade.
- Media import derived a region's timeline length inside the native insert
  call, re-validating the decode and rates on the way. Rust now reads the
  decoded frame count, rounds the length up itself and passes it through, so
  the native side only mutates storage.
- Articulation, expression-map, vocal-remover and correlation calls each
  walked the published audio snapshot to find their track. They share one
  snapshot lookup now, so the track-count and pointer-validity rules live in a
  single place.
- One-shot preview-sample playback kept its buffers, trigger/playing flags and
  cursor in C++ atomics, and the audio callback walked the published snapshot
  itself. Rust now owns the sampler behind an opaque handle: control code
  publishes and reclaims generations, and the callback only advances the
  cursor through one render call.
- Structural sync carried its own copy of the double-buffered audio-thread
  snapshot loop, so the track map and the 256-track cap had to be kept in sync
  in two places. Both paths now publish through the single locked helper, which
  also rebuilds the by-id map after track insert and removal.
- Sidechain route bookkeeping lived in a C++ vector that the layout writer,
  track duplication and track removal each had to walk and re-key by hand. Rust
  owns the route list now behind an opaque handle, so upsert, removal, capture
  and restore share one implementation and the engine only copies routes out
  when serializing.
- Structural sync rebuilt the delay-compensation graph from C++: it cleared the
  Rust store, replayed destination, send and latency setters, walked the
  connection snapshot twice and copied offsets back per track. Rust now performs
  the whole rebuild in one call from the published routing graph, so only the
  per-track latency snapshot and the resulting offsets still cross the boundary.
- Region, routing, control, plugin, automation and sandbox callbacks each
  walked the track list to resolve an id, with the same guard clause copied
  around. They now share one locked lookup on the engine, so a missing track is
  reported the same way everywhere and the traversal exists once.
- Ten region host callbacks repeated the same track/region lookup loop, so the
  matching rule lived in ten places. One locked-find helper now serves them all,
  which keeps every callback resolving the same region under the same lock and
  removes the chance of the traversal drifting apart.
- Device selection applied the negotiated format with two copies of the same
  quiesce-then-reconfigure transaction, one for CoreAudio and one for JACK.
  Both now call one helper, and the JACK graph sync uses it too, so a future
  change to that transaction cannot leave one driver path behind.
- The engine's eleven host tables are installed from one initializer instead of
  eleven per-table lazy statics. Installation still happens at image load before
  any caller can reach the engine, but the order is now explicit rather than
  left to cross-file static-initialization order.
- Duplicated and unreferenced declarations are gone. The JACK driver no longer
  defines `input_channel_count` twice (a latent build break for every JACK
  configuration), `Mixing::AestheticFeatures` and its `TelemetryData` field had
  no readers, and `MarkerInfo` was an unused bridge type. The articulation
  key-switch scheduler also moved out of `processBlock` into one shared helper,
  so the pre- and post-note-on transitions come from the same code path.
- Dead engine members are gone. `HirariUnifiedEngine::get_track` (whose
  fallback returned a function-local static `Track`), `Track::commitRegionEdits`,
  `BusTrack::fetchAudio`, `VideoSystem::frameRate` and the orphan
  `hirari_bus_track_fetch_audio` declaration had no remaining callers, so the
  engine surface now matches what the bridge actually uses.
- `AudioEngine` stopped duplicating engine getters that had no remaining
  callers. Five layout/generation readers now cross the cxx bridge bound to the
  engine itself, so Rust reads them without a forwarding hop, and the unused
  `is_loop_enabled`, `get_track_volume`, `get_master_gain` and `get_cpu_total_v`
  wrappers are gone.
- Dead C++ scaffolding left the engine boundary. The unused
  `PluginScanner` translation-unit fragment (its only entry point,
  `scanInSandbox`, had no callers — admission is decided by
  `hirari_plugin_scan_rejection` from Rust), `EngineOrchestrator`,
  `EngineDiagnostics` (its block counter was never read), the
  `readTempoSnapshot`/`readTempoEvents` helpers, and a batch of accessors with
  no remaining callers (`regionGeometryForRust`, `resetPluginControlPlane`,
  `getHeartbeat`, `takeProcessErrors`, `shouldScan`, `preFaderBuffer`,
  `postTaskAsync`, `DBRecord`, `WavDecoder::getFormatName`/`getBitsPerSample`,
  `MidiBuffer::addAllNotesOff`, `MetalAudioKernel::setEnabled`/`isEnabled`,
  and the bridge `StructureNode`/`PlainClash`/`PlainLoudness` types) were
  removed, along with the FFI declarations that only they referenced.
- Two unreachable engine entry points left the C++ boundary.
  `HirariUnifiedEngine::quantizeAudioGroup` (a 32-channel marshalling wrapper
  around `hirari_audio_quantize_group`) and the inline `get_dsp_load()` getter
  had no callers — group quantization is driven through the Rust
  `region_time_jobs`/`audio_quantizer` paths and DSP load reaches Rust through
  the telemetry snapshot. The now-unused `hirari_audio_quantize_group`
  declaration was dropped from the C++ header; the Rust entry point and its
  tests remain.
- The dead per-track C++ analysis wrapper is gone. `Track::AnalysisEngine`,
  its `getAnalysisEngine()` accessor, the `m_analysis` member and the
  `hirari_track_analysis_spectrogram` declaration had no callers; the Rust
  range-feature implementation and its tests remain the single authority and
  still expose the same C ABI for other consumers.
- Note-edit stores are cloned in Rust. The C++ `AudioNoteSegment` transfer
  type and its marshal/unmarshal helpers are gone: analysis snapshots deep-copy
  the store through `hirari_region_vari_audio_store_clone`, region-add detach
  copies the live table the same way, and project load feeds persisted notes
  straight into the flat note-store ABI. The unused `store_set`,
  `store_normalize`, `store_materialize` and `store_sizes` declarations left the
  C++ header; their Rust entry points remain for the bridge's own tests.
- Project saving shares the layout gather. `saveProject` used to walk the whole
  model a second time, filling the same encoder records C++ already produced
  for the layout JSON. The model walk now runs once
  (`collectProjectLayout`, which also keeps the save-only plug-in GUI blobs out
  of the polling JSON path) and Rust maps that snapshot onto the native
  container in `project_save_layout.rs`, including little-endian plug-in ids,
  store materialisation and catalogue bounds.

- The C++ dither façade is gone. `src/dsp/utils/dither.hpp` only wrapped the
  Rust `hirari_tpdf_dither_*`/`hirari_noise_shaping_dither_*` handles and had no
  callers, so the header and its include are removed along with C-ABI
  declarations that the engine never linked against
  (`hirari_bounce_output_path_supported`, the bounce-stream
  create/write/free trio, and the sidechain resolve/legacy-misuse probes).
  Streaming bounces still finalise through `hirari_bounce_stream_finish`.

- The plug-in fingerprint and undo-manager C++ contract blocks are gone.
  Native plug-in admission hashes paths inside Rust (`plugin_blacklist`), so
  the `hirari_plugin_fingerprint` C ABI, its `PluginScanCache::fingerprintForPath`
  wrapper and the duplicated native-contract assertions were dead weight; the
  Rust unit tests now cover content changes, missing files and
  filesystem-iterator ordering. Transactional undo likewise stays asserted in
  `undo_transaction.rs`, while the C++ `UndoTransactionManager` facade keeps its
  own closure-level contract.

- The test-only WAV/WAVE64 write, decode and diagnostic wrappers are gone.
  `WavSaver` and `WavLoader::load/loadLegacy/loadWave64` had no production
  callers, and their contract assertions (RIFF/RF64 headers, planar and
  streaming exports, NaN/Inf sanitising, WAVE64 round trips, oversize refusal,
  diagnostics and plug-in fingerprints) now run beside the Rust
  implementations. The native contract's XML escaping block moved to the Rust
  C-ABI test as well, which also repairs the contract the removed
  declarations had left uncompilable.

- The WAV write/decode contract now lives in Rust. Planar shape admission is a
  Rust predicate (`hirari_wav_planar_shapes_valid`) so `WavSaver::writePlanar`
  no longer re-derives the frame rule, and the decode/round-trip assertions
  (float and PCM round trips, on-disk RIFF sizes, NaN/Inf sanitising, odd JUNK
  chunks, RF64 sizes, truncated/corrupt headers) run against the Rust decoder.
  The two C++ contract translation units are gone.

- The sidechain snapshot contract now lives in Rust. Its assertions
  (registration admission, generation bump on refresh, level clamping,
  capture/restore, owned publication and the single-writer seqlock race) run
  against the Rust engine handle, and the 183-line C++ contract translation
  unit is gone.

- Single-project bounce orchestration now runs in Rust. The render driver owns
  extent planning, block chunking, pause/cancel, deterministic dither,
  progress/state publication and the publish boundary; C++ keeps the audio
  graph, transport atomics and streaming handles behind a host-ops table.

- Timeline markers and arranger parts are now stored by the Rust arrangement
  document. The C++ `ArrangementEditModel` is gone: marker ordering and
  replacement, arranger-part validation and sorting, and the storage itself
  live in Rust, and the project save path has the encoder read the document
  directly instead of mirroring it in the engine.

- Auto-mixing now runs in Rust. The equal-power target for the current track
  count, the 20% fader approach, the unity-range clamp and the bus skip are
  computed from the project layout instead of inside the C++ engine, so the
  engine no longer owns that mixing policy.

- The C++ `SidechainManager` adapter is gone. The engine, tracks and effect
  chains now call the Rust sidechain engine directly: link registration,
  silent links, per-block copy with metadata, source publication/refresh,
  track removal, and the undo capture/restore snapshot (including link
  validation) are all Rust-owned. This removes the third parallel copy of
  sidechain policy from the live engine and drops the C++ sidechain header.
- The differential-test oracles now live in Rust: all but one of the frozen
  C++ copies under `tests/dsp_reference` were ported to `dsp_reference/*.rs`
  and deleted — analysis, note curves, automation, MIDI buffers, bus/master
  output, limiter/de-esser/transient/chroma-glow, PDC delay, effect chains,
  region gain and stem splitting, masking analysis, warp mapping, the
  windowed-sinc resampler, pitch-correction delays, WSOLA grain selection, the
  polyphonic sampler, preview synth, holographic panner and spectral gain. The
  differential tests compare the same production Rust paths against these
  ports unchanged. Only the signalsmith-backed time-stretch oracle remains in
  C++ because it exercises the third-party engine directly; its
  arrangement-section generator also moved to Rust.
- Removed two engine methods that no caller in C++, Rust or the bridge still
  referenced (`set_track_solo_for_offline_render`, `get_track_count`). Offline
  solo selection and track counting already go through the Rust call sites.
- Read-only engine getters now go straight from Rust to the session engine:
  `AudioEngine` exposes its inner engine once (`get_core`) and the 28
  transport, plugin, sandbox and bounce getters that used to have a C++
  forwarder each are called on `HirariUnifiedEngine` directly. Fifteen
  `extern "C"` declarations that no longer had a C++ definition or caller were
  removed from `rust_ffi.hpp`.
- Swept the remaining C++ functions that nothing in `src`, `tests` or the Rust
  call sites referenced any more: sandbox health/reset helpers, automation and
  region-editing leftovers, arrangement/diagnostics conveniences and the
  unused compatibility-registry writers. No call site changed.
- Ten more engine relays now land in Rust directly: VCA group add/gain and
  snapshot JSON, region split, waveform/audio-range reads, sandbox
  maintenance, input-monitor channels and sidechain links. The C++
  `takeFloatBuffer` copy helper and the matching bridge declarations are gone;
  Rust owns the buffer hand-off. Orphaned C++ helpers with no remaining caller
  anywhere in the tree (dead `DSPUtils`, bounce/warp/freeze accessors, plugin
  scan wrappers) were removed as well.
- `rust_ffi.hpp` dropped the declarations and types that no C++ call site
  references any more: 58 `hirari_*` entry points, the alignment-target and
  region-scalar field types, and fourteen unused audio-graph, bounce, sampler,
  waveform and voice-callback contracts. The enums still read by the host
  (region edit ops, routing edit ops, automation edit kinds) stay.
- Twenty-three more two-layer engine edits moved into the same Rust inherent
  methods: region import queueing and source analysis, note-segment anchor
  and warp edits, region alignment/quantize jobs, plugin parameter writes,
  MIDI-CC macro bindings, route/send/feedback gains and expression-map
  install. Each pair of C++ relay layers and its bridge declaration is gone.
- Thirty-seven engine edits that were relayed through two C++ layers (an
  `AudioEngine` forwarder plus a one-line `HirariUnifiedEngine` delegation)
  now live as Rust inherent methods on the bridge type, calling the same
  `hirari_*` policy directly: track/plugin add-remove-duplicate, region
  move/remove/duplicate/trim/scalar edits, VCA gain reads, route and macro
  writes, expression-map Pro install, region import completion and the
  region source-accessor queries. Both C++ layers and their bridge
  declarations were deleted.
- All twelve built-in effect paths now resolve through the single Rust
  `NativeProcessor` dispatch: limiter, compressor, sub bass and saturation
  joined the gate/transient/de-esser/delay/reverb/dynamic-EQ/mid-side/width
  family, so their latency, parameter names, display names and byte-exact
  persisted state formats live in Rust and the four C++ adapter headers were
  deleted.
- The legacy C++ video frame cache is gone: nothing ever published into it,
  and the Rust video runtime already owns decoding and caching, so
  `get_video_frame` now only forwards the platform bridge result.
- The eight Rust-backed internal processors now share one `NativeProcessor`
  adapter driven by `hirari_native_processor_kind_for_path`, moving path
  matching, names, tail lengths, prepare/reset policy and parameter ids into
  Rust and deleting the per-effect C++ classes and FFI declarations.
- The WAV diagnostic JSON is produced by a single Rust entry point taking the
  file path, so `WavLoader` drops its C++ decode-then-format structured path
  and only copies the returned bytes.
- The diagnostic log ring is now a process-wide Rust singleton handed out by
  `hirari_log_buffer_default_state`, so the C++ `LogBuffer` adapter no longer
  owns its 128 KiB aligned storage or initialises the ring.
- The engine's hot-swap quiesce now reads a process-wide Rust hot-swap state
  through `hirari_hot_swap_default_quiesce_requested` and
  `hirari_hot_swap_default_acknowledge`, so the C++ `DiagnosticsKernel`
  facade, its callback plumbing and header are gone.
- The forensic journal writer now parks and wakes itself inside Rust, so the
  start entry point no longer takes host wake callbacks and the C++ adapter
  drops its atomic wake sequence.
- The bus summing system now owns its per-bus audio stages inside Rust and
  the engine keeps a single handle, so the C++ `BusSystem` adapter class and
  its header are gone.
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
