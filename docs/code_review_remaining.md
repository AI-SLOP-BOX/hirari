# Code review remaining work

This file records items that are not claimed as complete. A row is only
removed after an implementation and a matching verification command exist.

## Implemented and verified in the current workspace

- Project/audio generation checks at the command boundary.
- Atomic project persistence, recovery generations, stale-lock recovery,
  checksum validation, parent-directory syncing, and cleanup of abandoned
  per-project temporary files after interrupted saves, including the legacy
  project-manager compatibility writer. Save-lock owner records are flushed
  before publication and failed lock initialization removes its marker. Lock
  records now also carry a validated unique owner token; malformed legacy
  markers are not reclaimed, and new temporary paths carry the owner token.
- Session-owned PDC, undo, tempo, routing, sidechain, bus, macro,
  diagnostics, forensic journal, and audio decoder state.
- PDC configuration writers and recalculation now share a control-plane
  transaction lock; a cycle keeps the last valid active compensation snapshot
  instead of publishing zero offsets, and route repair republishes cleanly.
- PDC mutators can be bound to the control owner; once bound, calls from an
  audio-like thread are rejected before touching the configuration mutex.
- Sidechain registration now requires a bounded source block and copies it into
  fixed-address, control-owned ping-pong storage. Audio processing reads only
  an immutable published block, so source-buffer reallocation and writes in
  progress cannot invalidate the callback's pointers.
- Audio callback ingress rejects blocks above the bounded processing capacity;
  playhead/block-end and MIDI note-end arithmetic uses checked uint64 addition
  and exposes a sticky overflow diagnostic.
- Hot-swap quiesce acknowledgements count consecutive muted blocks and expose a
  budget query; the existing daemon deadline clears an unresponsive request.
- Realtime processing no longer constructs the forensic timer/message-bus
  telemetry path or lazily initializes the orchestrator singleton on the audio
  callback. Callback ownership tokens use a lock-free thread-local sequence.
- Bounce-in-place now exposes a session-owned `TimelineSystem` entry point;
  the singleton overload remains only as a compatibility facade.
- Multiple session graphs can be exercised in one process without the
  macOS test driver callback mutating the compatibility singleton; isolated
  native tests deliberately use offline session graphs.
- Sandbox state transition exclusion, checksum/version checks, overrun and
  quarantine handling, crash/restart paths, and structured diagnostics.
- `AudioProcessorGraph` now sanitizes non-finite samples at ingress before
  bypass/empty-graph paths, and the native graph contract verifies that
  NaN/Inf input cannot pass through unchanged.
- Waveform generation and parallel stem rendering now submit work to the
  bounded native `ThreadPool` instead of spawning an unbounded `std::async`
  thread per request; enqueue failure clears pending state and reports a
  recoverable waveform error.
- Parallel asset loading and bounce-in-place now use the same bounded
  `ThreadPool`, while preserving their `std::future` APIs for callers and
  generation/atomic-publication guards.
- Offline engine bounce temporary paths now include a monotonic publication
  token in addition to the worker identity, preventing recycled thread IDs
  from targeting a stale or newer render file.
- Native project hydration now rejects truncated plugin-data records, plugin
  state/bypass arrays that exceed the serialized plugin count, and sandbox
  state/path count mismatches; state application failures are no longer
  silently ignored during reload.
- Native project loading now enforces cumulative budgets before and during
  allocation: total file bytes, strings/paths, plugin payload bytes, plugin
  state bytes/entries, and automation points. The native contract also
  rejects a sparse project larger than the file budget before reading it.
- Native project reload now rejects zero/duplicate track IDs and regions that
  reference a missing track. The native contract covers duplicate-track and
  orphan-region rejection so reload cannot silently retarget edits.
- Render/export and waveform publication now share the canonical
  `GenerationGate` implementation; the former duplicate `PublicationGate`
  semantics are an alias with the same begin/cancel/accept rules.
- Audio-device transition coverage now includes a dedicated fallback and
  reconfiguration contract for sample-rate/block-size changes and invalid
  transitions. A long-duration stress target and a strict external-plugin
  matrix target are available without claiming success when required SDKs or
  hardware are absent.
- AU E2E now invokes `auval` for the concrete Apple component before running
  the mono/stereo native host contract; VST3 remains strict when its official
  SDK and fixture are required.
- The CMake verification surface now exposes a strict external-plugin matrix,
  a software audio-device transition contract, and a long-duration stress
  target that also exercises persistence and stale-render completion. The
  matrix never treats missing AU hardware, VST3 SDKs, or fixtures as a pass.
- Native WAVE64 parsing now rejects chunk-size alignment arithmetic overflow
  and padded chunk extents that exceed the file, closing a malformed-input
  wraparound path while retaining the existing WAVE64 diagnostics contract.
- The legacy void audio-config FFI entrypoint now applies the same finite
  sample-rate/block-size/tempo validation as the fallible path; invalid direct
  calls cannot quiesce callbacks or advance the audio configuration generation.
- The local stem-separation provider now uses the bounded native `ThreadPool`
  while retaining explicit unavailable-model failure; it no longer creates an
  unbounded `std::async` thread for each request.
- The command protocol now validates its outer protocol version, request ID,
  and client identity before command/schema/generation validation; malformed
  or untraceable wire requests fail with structured errors.
- Sandbox audio shared blocks now carry an explicit protocol version. The host
  publishes it after constructing the shared block and the worker rejects a
  mismatched ABI before touching audio state, returning a structured ABI error.
- The native plugin worker now rejects unknown options, missing values,
  malformed/overflowed file descriptors, non-finite sample rates, and invalid
  frame/channel ranges before opening shared memory or loading a plugin.
- AU parameter automation is queued off-thread and consumed in the render
  callback through realtime-safe `AudioUnitScheduleParameters`; queue loss is
  observable instead of falling back to `AudioUnitSetParameter`.
- RF64/RIFF validation, odd-chunk handling, malformed-input rejection, and
  canonical Rust export/reader tests.
- Native BWF offline rendering now accounts for the RF64 `ds64` chunk in its
  published `riffSize` while retaining BWF metadata.
- Vulkan shader build failures now expose stable `AURA_SHADER_ERROR` codes for
  compiler failure, compiler absence, invalid SPIR-V, and missing SDK/headers;
  enabling Vulkan without its compiler now fails closed instead of silently
  skipping shader validation.
- Vulkan-enabled native compilation now covers the split `VulkanContext` and
  `VulkanGraphicsKernel` implementation units; their conditional compilation
  boundaries are closed within each included unit and verified with
  `AURA_ENABLE_VULKAN=1 cargo check -p aura-core-bridge`.
- Render and sandbox completion checks use bounded job/sequence polling;
  remaining sleeps are only polling intervals with explicit deadlines.
- Release fixture generation, strict bundle verification, native compile
  contract, realtime-boundary audit, and session-singleton audit.
- CMake release architecture selection now derives from the configured target
  architecture and recognizes arm64+x86_64 macOS builds as `universal2`,
  avoiding host/Rosetta-derived release metadata.
- The realtime-boundary audit script is executable for direct local/CI use;
  it passes alongside the session-singleton audit.
- The deterministic project/WAV/plugin-state fuzz smoke script is also
  directly executable and passes its truncated-input corpus.
- Headless release smoke waits for delayed worker/shared-memory cleanup before
  declaring shutdown successful.
- Waveform generation now flushes a partial final pixel at stream finalization,
  preserving tail peaks instead of silently dropping them.
- Stale waveform workers now always release their generation-owned pending
  slot on both success and failure; stale results remain barred from cache
  publication while a newer request can proceed. The waveform generation
  contract covers this invalidation/re-request sequence.
- Asynchronous asset, waveform, and stem jobs retain their futures until
  completion/reaping; no product path relies on a discarded temporary future
  to manage worker lifetime.
- Cache publication now holds the per-path generation lock across the final
  generation check and rename, preventing an older worker from publishing
  over a newer cache between those operations. The native contract exercises
  two successive writes and verifies the newest payload wins.
- Recording `start()` and `stop()` now serialize control-thread lifecycle
  operations around writer-thread drain, header patching, and temporary-file
  publication; the realtime `write()` path remains lock-free.
- `ProjectDB` now preserves the last destination when atomic rename fails and
  cleans only its temporary file; the native contract forces a publication
  failure against an existing directory and verifies that destination remains.
- `AudioExportEngine` temporary output names now include process identity and
  an atomic sequence, so concurrent exports from one thread cannot share a
  temporary path.
- `AudioExportEngine` now treats parent-directory fsync failure after publish
  as an export failure instead of reporting durable success without a synced
  directory entry.
- Canonical native WAV/RF64/WAVE64 publishers now share a strict parent-
  directory sync helper; inability to open or fsync the directory is reported
  as failure instead of silently claiming durable publication. The legacy
  `WavSaver` interleaved path applies the same rule.
- `RecordingEngine` now uses the canonical bounded-memory IEEE-float stream
  publisher for its realtime-to-disk path. The recorder no longer owns a
  second float-WAV header/finalization implementation; RF64 selection,
  exclusive temporary publication, file sync, and parent-directory sync are
  shared with the rest of the product writers. The native contract now runs a
  real recorder start/write/stop cycle and checks the resulting float WAV
  header and exact file size.
- Async project serialization and the Rust-facing persistence serializer now
  also fail explicitly when the published file's parent directory cannot be
  opened or fsynced; the last published project remains intact.
- Native project asset collection now rejects symlink sources, uses deterministic
  hash-suffixed names for basename collisions, syncs each temporary file before
  publication, syncs the Assets directory after rename, and rolls back already
  published files on any later failure. The native contract covers missing-input
  rollback, collision preservation, and symlink rejection.
- Native sandbox test isolation records its owner PID and reclaims stale lock
  directories after interrupted test processes, so an aborted stress/test run
  cannot make later tests spin forever on the old lock.
- Shared-memory telemetry now carries an explicit protocol version; creators
  publish it after placement construction and `openExisting()` rejects an
  incompatible segment before exposing it to callers. The native contract
  also creates a stale same-sized segment and verifies rejection without
  consumer-side unlinking.
- Native WAVE/WAVE64 reads now have a structured diagnostic result and a
  dependency-free JSON diagnostic boundary; malformed, missing, and valid
  WAVE64 cases are covered by the native compile contract without changing
  the legacy throwing loader APIs.
- The Rust/CXX bridge exposes the native WAVE diagnostic to `AuraCore`, with a
  Rust integration test preserving the missing-file reason and format.
- Canonical native persistence now provides a bounded-memory PCM24 streaming
  publisher with RF64 headers, fsync, directory sync, atomic rename, and
  temporary-file cleanup; its round-trip/header contract is covered by the
  native contract, and `src/core/io/bounce_system.hpp` now uses it for the
  product render path.
- The native contract now executes a short `BounceEngine` render and verifies
  that the migrated path publishes a valid RIFF/WAVE file.
- The source-compatible `LegacyBounceEngine` PCM16 callback path now uses the
  canonical bounded-memory PCM16 stream publisher; its progress, cancellation,
  publication, and PCM16 header contract are exercised natively.
- `OfflineRenderer` now uses the same canonical PCM24 stream publisher with
  the BWF `bext` metadata option, preserving its RF64/BWF header contract while
  removing the last product-local temporary-file writer.
- `BounceEngine::renderMaster` now streams the standard IEEE-float WAV path in
  bounded blocks instead of retaining an entire song in RAM; optional mastering
  advice uses a capped downsampled analysis buffer so long bounces keep bounded
  memory without changing the published audio payload.
- The native contract now checks the BWF `bext` chunk, fmt/data offsets, and
  payload start rather than only checking the RIFF magic bytes.
- Autosave flushes now serialize concurrent callers through a dedicated save
  mutex, wait for atomic publication, and are covered by an 8-way concurrent
  flush contract that checks complete JSON plus temporary/lock cleanup.
- Recording publication no longer deletes the last completed output as a
  fallback when replacement rename fails; it now preserves the prior file and
  reports the publish failure, with the native contract still compiling the
  recording path.
- Plugin preset publication now uses process/sequence-unique temporary files,
  fsyncs the payload before rename, and syncs the parent directory so
  concurrent preset saves cannot collide or publish an unflushed state.
- Project v2 hydration now propagates native rollback failures into the
  returned error instead of silently discarding the restore result; every
  incremental track/plugin/region failure path uses the same rollback wrapper.
- Undo hydration now reports the native restore result and only moves the
  current snapshot to the redo stack after a successful restore; a failed
  restore keeps the original undo entry available instead of consuming it.
- Preset temporary-file naming now has an explicit Windows-safe branch instead
  of requiring POSIX `getpid()`, while retaining process/sequence uniqueness on
  the native Unix path.
- Waveform cache failures now retain a per-region/per-resolution reason for
  UI consumers; successful regeneration, external cache insertion, and
  invalidation clear that keyed failure state.
- Legacy Bounce-In-Place now accepts an explicit `OutputFormat::WAVE64`
  selection and routes it through the canonical WAVE64 writer; its header
  dependencies are included in the native compile contract.
- Native project publication now treats the parent-directory sync as part of
  the atomic-save contract: a missing or unsyncable parent directory makes
  `saveAtomic()` fail instead of reporting success after the file rename.
- Plugin admission-cache and blacklist publication now return a failure when
  the post-rename parent-directory sync cannot be opened or completed; callers
  intentionally discard the status because these are background maintenance
  operations, but they no longer silently report a durable publish.
- CoreAudio driver lifecycle now has explicit mutex-header hygiene in the
  standalone driver contract; device-loss recovery already stops transport,
  quiesces callbacks, resets captured input, and prepares the engine with the
  newly negotiated sample rate/block size before restarting.
- Sandbox recovery now has an executable contract for the complete fallback
  policy: a bounded mailbox deadline produces finite cleared audio, repeated
  overruns quarantine the processor, restart clears the recovery latch, and
  the first post-restart audio/MIDI block is checked for exact-once delivery.
- `AudioExportEngine` stereo PCM16/PCM24 publication now delegates to the
  canonical persistence writer, removing its product-path-specific header,
  rename, and durability behavior. Mono remains on the compatibility path
  until a canonical interleaved PCM16 API exists, so it is not falsely treated
  as complete.
- Canonical persistence now also provides interleaved PCM16 output for mono
  and multichannel layouts, including RF64 sizing, finite/clamped samples,
  atomic publication, file sync, and parent-directory sync. The native
  contract decodes a three-channel PCM16 file and verifies channel order and
  sample values.
- `AudioExportEngine` now routes mono and stereo PCM16/PCM24 output through
  the interleaved canonical APIs; its old local writer remains unreachable
  compatibility source rather than a product execution path.
- The native WAVE decoder contract now exercises the same odd-sized unknown
  chunk padding, RF64 `ds64`, PCM16, malformed-header, and finite-sample rules
  alongside the higher-level loader and random-access reader. The remaining
  decoder duplication is intentional: full decode, streaming/mmap random
  access, and bounded import diagnostics have different ownership/performance
  contracts.
- Rust asset consolidation and resource copying now sync the temporary payload
  before rename and the containing directory after rename; a failed durability
  step removes only the unpublished temporary file. The existing content-hash
  collision and temp-cleanup test still passes.
- The CXX audio configuration boundary now exposes `try_apply_config`; it
  rejects invalid tempo/sample-rate/block-size values before waiting or
  changing the engine generation, while preserving the legacy void API for
  compatibility. The generation test covers rejection without advancement and
  successful reprepare advancement.
- The project layout generation is now exposed as a stable FNV-1a token through
  the native and Rust/CXX bridge APIs. `push_command` reuses the same helper,
  so UI/CLI snapshots can echo their generation and stale structural commands
  are rejected consistently at the enqueue boundary.
- Command enqueue now checks the bounded SPSC ring-buffer result. A full
  command queue returns failure and emits a diagnostic instead of falsely
  acknowledging a command that the audio callback will never see.
- Command rejection now exposes a stable numeric reason through the native and
  Rust/CXX bridge: invalid target, stale project generation, stale audio
  generation, or queue full. Successful enqueue clears the reason, allowing
  UI/CLI callers to decide whether to refresh a snapshot or retry.
- Async project serialization now keeps its in-flight flag set through the
  containing-directory `fsync`, not only through the file rename. A subsequent
  autosave/manual save cannot overtake a publication whose durability barrier
  has not completed.
- Recording publication now treats a missing or failed parent-directory
  `fsync` as a write failure instead of silently reporting a durable recording
  after only the file-level sync succeeded.
- Track PDC latency changes now preserve delay-line history and crossfade the
  previous tap into the new tap over 64 samples. A latency update no longer
  clears the line and injects a full-block silence discontinuity.
- Plugin state IPC now uses a 64-bit FNV-1a checksum instead of a 32-bit
  checksum. The shared audio-block ABI version is bumped so an older worker
  cannot interpret the changed state field layout; host and worker continue to
  reject unsupported state versions and checksum mismatches.
- Project hydration no longer registers provisional BusTracks in the shared
  BusSystem before validation completes. Bus resolution is deferred until the
  commit point, so a failed hydrate cannot leave routing/bus presence side
  effects behind the previous project.
- Plugin cache loading now re-applies the scanner's admission gate before
  accepting a persisted entry. A cache file cannot bypass symlink, regular
  file/directory, or AU/VST3/CLAP extension validation; fingerprints still
  invalidate changed binaries afterward.
- Audio reconfiguration now closes callback admission before waiting for
  in-flight callbacks and rebuilding master/track buffers. The new generation
  is published only after preparation and structural sync finish, preventing
  buffer reallocation from racing an audio callback.
- Native PCM16/PCM24 compatibility writers now retain and report atomic
  publication errors instead of reusing a cleanup error-code slot, while
  still removing failed temporary files.
- Rust streaming recording publication now uses no-replace semantics on Unix
  instead of a check-then-rename sequence, preventing a late recorder from
  overwriting a newer take; a regression test preserves the existing output.
- Rust WAV/RF64/WAVE64 export paths now create their temporary files with
  `create_new`, so a timestamp/PID collision fails safely instead of
  truncating another export's temporary file; all export format tests remain
  covered.
- Native WavWriter legacy PCM16/PCM24/WAVE/WAVE64 paths now reserve temporary
  names with exclusive creation before opening streams, preventing stale-name
  cleanup and truncation races across concurrent exporters.
- Persistence backup rotation now stages and syncs the next `.bak.1` before
  removing or shifting older generations. A failed source copy can no longer
  erase the last recoverable backup; staging names are also created
  exclusively.
- The legacy Rust resource manager now copies into an exclusively-created,
  synced temporary file before publication, preventing temporary-name
  truncation and preserving cleanup on partial asset copies.
- The second Rust `resources.rs` asset path now follows the same exclusive
  copy, file sync, parent-directory sync, atomic rename, and rollback
  contract; basename collisions remain content-addressed.
- The canonical Rust `asset.rs` consolidation helper now performs an
  exclusively-created streaming copy, syncs the payload and parent directory,
  and removes partial output on failure.
- Recording session finalization failures now move the session back to a
  consistent idle/failed state and clear stale region/path references instead
  of leaving a stopped preview marked as actively recording.
- Region ID exhaustion now returns a rejected add operation instead of
  panicking, and the exhaustion path is covered without allocating a track
  vector.
- Recording publication no longer relies on an `Option::unwrap` after spool
  selection; the selected spool is pattern-matched through the path-building
  branch so a concurrent state change fails closed.
- Region recording publication now has no production unwrap at the spool
  selection boundary; invalidated capture state fails through the existing
  error path instead of panicking.
- Runtime WAV validation now performs all `ds64` 64-bit reads through checked
  conversions, and malformed/truncated headers are covered by a no-panic
  rejection corpus.
- Macro and preview-synth diagnostic FFI mutations now use explicit engine
  presence branches rather than null-check-plus-`expect`, preserving the
  structured unavailable-engine response under teardown races.
- Track scalar, track-name, region-add, and region-replace diagnostic FFI
  mutations now use the same explicit engine presence branches; no engine
  pointer is unwrapped after a separate null check.
- Region/track diagnostic helpers, video load/frame requests, and render
  start/cancel diagnostics now use the same explicit engine branches, removing
  the remaining null-check-plus-`expect` paths in that FFI module.
- Mixing-advice diagnostics now use the same explicit engine branch, so the
  FFI module no longer relies on a null-check-plus-`expect` for its remaining
  auto-mixing mutation.
- The compatibility project-manager save lock now records its owner PID and
  creation time, reclaims only a provably dead Unix owner, and fails closed for
  empty or malformed lock contents; a regression test covers stale-owner
  recovery without weakening active-writer serialization.
- The metronome's infallible default constructor no longer depends on a
  production `expect`; its validated default state is constructed directly,
  keeping startup free of an avoidable panic path.
- `StreamingBuffer::getSamples` no longer calls a condition-variable
  notification from the audio thread.  The producer follows the atomic SPSC
  indices instead, leaving OS wait/wakeup primitives exclusively on the
  background I/O side.
- Native bridge save rollback and sidecar backup copies now use exclusive
  temporary files, file `sync_all`, atomic rename, and parent-directory sync;
  direct `fs::copy` overwrite paths were removed from the primary save failure
  recovery flow.
- The native compatibility `ProjectManager` no longer silently keeps a
  different asset when two regions share a basename.  Equivalent files are
  reused; different content receives a deterministic fingerprint suffix and
  collision index before copying with non-overwrite semantics.
- The legacy `AssetManager` now rejects symlink/non-regular inputs, preserves
  distinct same-basename assets with deterministic suffixes, and fails closed
  when a requested project-local copy cannot be published instead of silently
  retaining an external reference. Its project-local publication also uses a
  fsynced temporary file and atomic rename.
- `AudioExportEngine::bounce` no longer carries an unreachable second writer
  and publish path after the canonical `WavWriter` return. The product path now
  has one visible export implementation, reducing the chance that a future
  change updates a dead legacy branch instead of the actual renderer.
- Native `ProjectManager` asset publication now writes through a unique,
  fsynced temporary file and atomically renames it into `Assets`, with parent
  directory sync and cleanup on failure; partial copies are not exposed as
  project assets.
- The native `ProjectCollector` temporary publication names now include the
  process identity as well as the sequence, preventing separate Aura
  processes from colliding on a shared project directory during collection.
- The native plugin contract now exercises project collection with two
  different same-basename assets and verifies that both published files
  survive; this guards the collision policy at runtime rather than only at
  compile time.
- Command API validation now rejects mutation batches declared with
  `read_only` permission while preserving generation-free project inspection;
  regression tests cover both paths.
- Standard project loading now snapshots native, comping, and MIDI state
  before hydration and restores all three when native or sidecar loading
  fails, preventing a partially loaded project from becoming visible.
- The sidecar failure rollback path is covered by an integration test that
  proves the pre-load native layout survives a malformed MIDI sidecar.
- PCM16/PCM24 streaming writers now reserve temporary paths exclusively
  instead of deleting stale names before opening; the native WAV contract
  verifies reservation collisions, and `WavSaver` rejects empty or mismatched
  channel buffers before publication.
- `PDCManager` now tags routing/latency changes with a configuration
  generation and discards a recalculation if inputs changed before publish;
  stale offsets and cycle flags cannot be published from a mixed graph snapshot.
- The sandbox IPC contract now makes the one-block deadline deterministic and
  verifies that an in-flight block's pending audio/MIDI is not overwritten by
  later polling calls.  The optional CLAP fixture proves the recovered block
  returns exactly one echoed MIDI event; the builtin worker remains an
  audio-only fallback when fixtures are unavailable.
- Rust project saves now use exclusive temporary creation, and the legacy
  `rotate_existing_backup` entry point participates in the same cross-process
  save lock as normal saves, closing a backup-rotation race between manual and
  autosave callers.
- `ModularGraphOrchestrator` now treats corrupted public edges and impossible
  in-degree transitions as invalid topology instead of calling `unwrap()`;
  regression tests cover invalid edges and cycles without panicking.
- `PluginHostInfrastructure::createProcessor` now re-runs the canonical
  admission/fingerprint/blacklist gate immediately before external sandbox
  creation. A plugin binary changed after registration can no longer bypass
  admission merely through the execution path; the native contract verifies
  that it is rejected and requires a rescan.
- CoreAudio input capture now refuses to enqueue blocks while the driver is
  stopping or not running. Device-loss callbacks therefore cannot leave stale
  input blocks in the queue for the next configuration generation.
- The native compatibility `get_track()` path no longer redirects an unknown
  or stale track ID to the first live track. It returns only the explicit
  invalid fallback, while command insertion continues to reject missing IDs;
  this prevents post-reload edits from silently targeting the wrong track.
- MIDI snapshot replacement now has a structured diagnostic API that
  distinguishes malformed JSON, invalid events, normalization rejection, and
  poisoned state locks. Regression coverage verifies malformed and invalid
  snapshots without relying on a silent legacy boolean.
- AU state restore and reset now quiesce the audio callback before touching the
  AudioUnit, preserve an existing user/watchdog bypass state, and expose
  restore failures to the control side. ClassInfo state is serialized through
  CFPropertyList binary data and decoded back before restore, rather than
  passing an opaque CFData wrapper. The Apple AU smoke exercises mono and
  stereo render, parameter scheduling, state round-trip, reset, and
  oversized-state rejection.
- The canonical native WAV decoder rejects declared decoded payloads above
  512 MiB and checks `std::streamsize` conversion limits before allocating or
  reading the payload. RIFF chunk traversal now also enforces the declared
  RIFF boundary, padded chunk arithmetic, and rejects chunks extending beyond
  the container; the native WAV contract covers that malformed-header case.
- Plugin cache fingerprints now use deterministic byte-wise FNV input and a
  sorted recursive path list rather than implementation-defined `std::hash`
  or filesystem iteration order. The native contract recreates the same
  directory in opposite creation order and requires identical identity.
- The macOS `CoreAudioDevice` now serializes initialize/start/stop/reconfigure
  and protects lifecycle-owned getters from concurrent teardown. Failure
  cleanup uses the unlocked path to avoid recursive lifecycle locking; the
  real CoreAudio transition contract still passes.
- Plugin cache and blacklist maps now use one canonical filesystem key for
  load, freshness, admission failure, and recovery operations. Relative,
  absolute, and `./` aliases can no longer create separate cache identities;
  the native contract verifies alias lookup and cleanup.
- Persisted blacklist entries are canonicalized on load as well, so upgrading
  from an older cache format cannot accidentally bypass a stale admission
  block merely because the saved path used a different spelling.
- Plugin fingerprints now canonicalize the root path before hashing and use
  content identity rather than file/directory mtimes. Equivalent bundle copies
  therefore produce the same admission key, while byte changes still
  invalidate the fingerprint. The native contract covers both deterministic
  directory recreation and alias-based blacklist lookup.
- CoreAudio now accepts the configured mono AudioBufferList shape instead of
  requiring two buffers, and reports a disconnected render path as unavailable
  silent fallback. The macOS device contract starts a real mono path before
  reconfiguring to stereo.
- Rust stale-save-temp recovery now checks the PID encoded in each temp name
  before deleting it. A correctly formatted temp owned by a live save process
  is preserved; only dead-owner temps are reclaimed. The persistence contract
  models both cases explicitly and all persistence tests pass.
- Plugin scanner results and admitted plugin descriptors now expose canonical
  paths and deterministic ordering, keeping scanner, admission, cache, and
  execution identities aligned.
- Native stress no longer maintains an unused global worker-name baseline;
  worker leak checks remain scoped to the PID descendants of each test command,
  avoiding false ownership claims from unrelated Aura sessions.
- History checkout/restore now holds the history transaction lock across
  project hydration and HEAD publication, restoring the previous project bytes
  if HEAD publication fails. Revert and cherry-pick use the same atomic
  working-tree-plus-new-commit path, so the CLI no longer leaves a silently
  detached project snapshot behind the current branch.
- Canonical `ProjectDocument` files now carry a UUID independent of their path.
  Existing-project saves preserve that identity, first history commit adopts it,
  and replacing a committed project at the same path fails closed instead of
  silently mixing histories.
- OpenUtau source/render pairs are now held on the Aura control plane and copied
  into canonical ProjectDocument saves/loads instead of existing only as a
  transient region import. CLI project inspection reports the persisted vocal
  count, with a regression test covering the save boundary. Imports now retain
  SHA-256 source/render identities and WAV shape metadata, making an external
  re-render or replacement observable instead of silently changing the sound.
- Canonical saves now project every native plugin slot into
  `PluginInstanceContract` with stable track/slot identity, format, state blob,
  capability, and (when the external binary is present) a content fingerprint.
  This prevents the formal plugin model from diverging silently from the
  legacy native layout during save/load.

## Remaining implementation work

| Area | Current status | Required evidence before claiming complete |
| --- | --- | --- |
| WAVE64 | Rust export has an atomic IEEE-float WAVE64 writer/reader and explicit orchestrator entry; native writer/reader are covered by the contract, native offline bounce format `1` and `BounceEngine::Format::WAVE64_32F` select WAVE64, Rust bounce validation/diagnostics recognize it, native WAVE64 supports validated interleaved multichannel output through both the persistence facade and `WavSaver::saveWave64`, native reads expose structured JSON diagnostics, and the master bounce now uses a bounded-memory WAVE64 stream writer with round-trip coverage. | Keep remaining legacy compatibility callers on the canonical WAVE64 API. |
| Native WAV writers | The `Core::Utils::WavWriter` compatibility facade, `RecordingEngine`, all `WavSaver` channel layouts, `AudioExportEngine`, and bounce paths now delegate to canonical persistence APIs; PCM16, PCM24, multichannel PCM24, IEEE-float recording, stereo WAVE64, and multichannel WAVE64 paths are covered by the native contract. The decoder contract also covers unknown odd-sized chunks and padding. `BounceEngine` PCM16/PCM24/float32 master output uses bounded streaming writers; MP3/FLAC are published through the separate validated external-codec path. | Keep every product path on one canonical writer API and extend the contract for WAVE64 streaming and any future codec backend. |
| External codec export | BounceEngine now stages bounded float32 WAV output and publishes MP3/FLAC only after the allowlisted FFmpeg encoder succeeds; FFmpeg is launched without a shell, failed encoding never replaces the destination, and ExportManager cancellation propagates through rendering and terminates the POSIX encoder child. | Verify FFmpeg availability and codec output in a release environment; add encoder progress reporting and Windows process cancellation. |
| External codec import | MP3/FLAC/AIFF/AIF/M4A/OGG/AAC imports now use the same shell-free FFmpeg boundary, decode to a unique temporary WAV, and pass through the bounded canonical WAV decoder; failed conversion cannot publish a partial asset. | Add a Windows process adapter and retain explicit unsupported status when FFmpeg is unavailable. |
| VST3 | Official SDK host build and Surge XT fixture E2E pass locally through `scripts/run_vst3_e2e_smoke.sh`: instantiate/process, reconfiguration, instrument state restore with finite note audio, crash/quarantine, and overrun recovery with MIDI checks. | Pin SDK/fixture provisioning in CI and repeat on the supported release matrix. |
| AU component matrix | Basic AU smoke exists; multi-component fixture coverage is incomplete. | Mono/stereo, effect/instrument, fallback-component, state and watchdog fixture tests. |
| MIDI 2.0/SysEx | Bounded SysEx fragmentation/reassembly and strict UMP packet framing validation now exist with ordering, size, word-count, message-type, and reset tests. Native code now has a heap-free bounded `MidiFragmentReassembler`; completed messages that fit the 256-byte realtime slot are inserted into `MidiBuffer`, while larger messages publish to a bounded SPSC `MidiExtendedMessageRing`. The ring is now embedded in `SharedAudioBlock`; the sandbox worker consumes bounded messages and exposes them as CLAP MIDI-SysEx events, and the minimal CLAP fixture E2E now injects a 300-byte payload and verifies the resulting audio response. Legacy MIDI remains covered separately. | Real third-party MIDI 2.0/SysEx compatibility and AU/VST3 event mapping still require SDK/device fixtures. |
| Real device transitions | Software fallback and lifecycle guards exist; macOS now has a real CoreAudio initialize/start/stop/reconfigure contract and the release gate requires it. | Hardware disconnect/reconnect, recording and PDC integration tests still require a dedicated device fixture. |
| Long-duration stress | Native lifecycle/resource-budget stress exists, persistence has a 256-generation repeated save/load soak with temporary-file leak assertions, and native worker stress supports either a round budget or `AURA_STRESS_DURATION_SECONDS` for bounded time-based runs. The worker matrix covers continuous blocks, reconfiguration, project-v2 state restore, multi-instance isolation, overrun recovery, crash/restart, and quarantine. A 5-minute local time-budget run completed 87 worker rounds, and a 5-minute recording stress completed with finalized-frame and temporary-file checks. The test lock records an owner PID and recovers stale locks after interrupted runs. The CMake `aura-test-project-soak` gate runs an explicit 64-track, eight-cycle save/reload/render graph-preservation test. | Multi-hour/large-project soak with waveform, render, save and device transitions. |
| macOS multi-session audio callback | Test isolation now prevents the legacy device callback from binding multiple session graphs to the process-wide compatibility engine. | Replace the compatibility callback with an injected session callback before claiming simultaneous real-device sessions. |
| FFI return-type migration | Structured diagnostics now cover project, bounce, sandbox, driver, track scalar/fader/pan/toggle/EQ/macro mutations, track lifecycle, project scale, vocal remover, articulation map, mixing advice, auto mixing/arrangement, undo/redo, transport playback/playhead/loop/test-tone, MIDI note/clearing/swing/humanize, recording-take selection, comp segment/snapshot validation, plugin preset save/load, and region add/replace/edit controls, plugin add/remove/bypass, plugin state, route/feedback/sidechain validation, audio device configuration/reconnect, automation data, plugin parameters, tempo controls, preview synth selection/pad assignment/scan/preload/trigger, and video frame/load requests; state size/error boundaries now have explicit regression tests; legacy bool/void APIs remain for ABI/UI compatibility. | Migrate any remaining analysis-only mutators without breaking the CXX ABI. |
| Render/record fixed sleeps | Completed for the current Rust integration paths. | Keep the bounded-deadline audit passing when new workflows are added. |
| Local AI stem/mastering runtime | Fails closed with an explicit unavailable-model error; no fake inference is reported. | Bundle and verify a real CoreML/Demucs provider, then add deterministic audio-quality fixtures. |
| Legacy native bounce writer | All three native paths now use canonical streaming publishers: PCM24 product bounce, PCM16 compatibility bounce, and BWF/PCM24 `OfflineRenderer`. They retain bounded memory, atomic publication, fsync, directory sync, cancellation cleanup, and native output contracts. | Keep the compatibility and BWF contracts covered as formats evolve. |

## Local verification baseline

The following currently pass locally:

```text
cargo check -p aura-core-bridge
cargo test -p aura-core-bridge --lib    # 346 passed
scripts/audit_session_singletons.sh
scripts/check_realtime_boundary.sh
scripts/run_native_plugin_compile_contract.sh
plugin_sandbox_ipc_contract (builtin and minimal-gain CLAP modes)
```

The current full CMake integration gate also passes locally: Rust/UI
integration tests, native compile contracts, ASan/UBSan, TSan, realtime and
async-generation audits, long-duration recording stress, native worker
lifecycle stress, fuzz smoke, CLI/history/render E2E, and the 64-track
save/reload/render project soak. The strict release bundle gate additionally
passed on the attached Apple Silicon Mac with architecture, codesign,
headless-app, dependency, and packaged-worker smoke enabled.

The absence of a third-party physical-device matrix remains an environment
limitation, not a passing result for that row. The macOS CoreAudio contract,
Apple AU validation, and an official-SDK VST3 fixture (Surge XT) were executed
locally. CI still remains fail-closed unless it provisions the same SDK and
fixture explicitly.

The current local stress run also completed successfully: 201 CLAP/VST3 worker
lifecycle rounds covered continuous processing, reconfiguration, crash/
quarantine, mailbox overrun recovery, and resource sampling. This is bounded
worker-lifecycle evidence, not a substitute for the multi-hour large-project
and physical-device matrix listed above.

The asynchronous publication audit is now a first-class CMake/CI check. The
autosave, offline render, parallel bounce, and waveform cache publishers must
use the canonical `GenerationGate`; stale-completion rejection remains covered
by unit tests.

The direct FFmpeg codec contract is also a first-class CMake target. On the
attached macOS machine it generated and identified both MP3 and FLAC outputs
through Aura's argv-based launcher using paths containing spaces and shell
metacharacters. The release gate now includes this contract through
`aura-test-all`; codec availability and output-format validation remain
environment-specific release requirements.
