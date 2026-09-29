# Hirari DAW Architecture Status

This document records the current production source of truth after the
superseded native UI paths were retired. It is intentionally explicit about
what is canonical and what remains compatibility or contract-test code.

## Runtime source of truth

| Area | Canonical path | Compatibility / test-only paths |
| --- | --- | --- |
| Application | `hirari-ui` Rust/Slint binary | Historical app bundles outside `packaging/` |
| Project model | `hirari-core-bridge::ProjectDocument` | Native serializer used for rollback snapshots and legacy import |
| Runtime engine | `hirari-core-bridge::HirariCore` + its owned native `AudioEngine` | Header/translation-unit contracts under `tests/` |
| Native audio graph | The `AudioEngine` owned by one `HirariCore` session | Older direct `HirariUnifiedEngine` entry points |
| External plugins | Isolated worker process and format adapter | Legacy in-process host wrappers |
| Release bundle | `packaging/Hirari DAW.app` | Historical bundles are not release inputs |
| Release gate | `scripts/verify_release_bundle.sh` via `hirari-verify-release` | Ad-hoc smoke scripts are capability tests |

## UI and rendering split

The desktop shell is Slint. The default Cargo configuration selects Slint's
WGPU 29 backend and installs a rendering notifier that uses the same device and
queue as the Slint frame. Shared plot frames are uploaded as RGBA textures and
imported back as Slint images for Arrange, Mixer, Synth, Waveform, Mastering
and Piano Roll surfaces. An opt-in `gpu-canvas` feature keeps a standalone
native `wgpu` 0.20 WGSL pipeline for renderer-level tests and integrations;
it is not a second window. Slint continues to own layout, accessibility and
input while dense plot pixels are produced by the GPU path. The separate C++
graphics layer under `src/graphics/` remains a compatibility path for Metal
and opt-in Vulkan experiments. The notifier tracks waveform, spectrum, meter,
and piano-note streams independently, so a meter-only update does not rebuild
the other three textures.

The active UI and graphics support have separate owners:

- `hirari-ui/` is the Rust/Slint application crate in the Cargo workspace. It
  owns the production desktop UI and invokes the engine through
  `hirari-core-bridge/`.
- `src/graphics/ui_components/support/` contains C++ helpers used by the
  graphics components and native contract tests. They are not a second
  application UI.
- The superseded native C++ window and workspace tree is archived under
  `archive/legacy-cpp-ui/`; it is not compiled by the production bridge build.
  The former root-level `src/ui/` directory has been retired.

`hirari-ui/` was already the production UI crate at the repository's current
base revision; the Hirari work renames that existing crate rather than adding
a second UI implementation. The root `src/` is not an alternative Rust UI
location: it is the C++ engine tree. The repository history does not record the
original decision to split the Rust UI into its own crate, but the current
build boundary is the Rust/Slint app, a Rust/C++ bridge, and the C++ engine.

DSP migrations use C++/Rust equivalence tests as a completion gate, not just a
successful build. Current coverage and remaining reference fixtures are
tracked in [Rust migration equivalence gates](./RUST_MIGRATION_EQUIVALENCE.md).

## Project lifecycle rules

1. A project load is a control-thread transaction.
2. All filesystem inputs are validated before mutating the native graph.
3. Native hydration creates a rollback snapshot before the first mutation.
4. Any failed track, region, parameter, or layout restore rolls back the
   native graph and reports failure to the caller.
5. A successful load removes the rollback snapshot.
6. Audio configuration and sandbox IPC generations must not be reused by a
   later project session.

The live `.hirari` document is the canonical project model. The existing
`ProjectArchive` now creates a single-file `.hirari-package` ZIP containing
the project JSON, a manifest, and referenced media; restore verifies checksums
and extracts to a new project directory before opening it. This is the
portability/collection operation, not a replacement for the lightweight live
save and backup format. Current JSON `.hirari`
saves contain MIDI notes/events, comping, and Control Room in one atomic
document; duplicate current-generation `.midi.json` and `.control-room.json`
files are no longer published. Loaders keep migration support for older
sidecars, and the legacy ARUA/native reader still handles its MIDI-event and
comping sidecars. Audio and freeze media remain referenced external files, so
live `.hirari` is not a self-contained media bundle. UI project loads hydrate
a staging track model and publish it only after Core load plus UI hydration
succeed; failed loads restore Core and then publish the matching UI rollback.

## Release capabilities

The release bundle currently has a mandatory CLAP worker smoke. AU and VST3
are capability-gated because their SDKs, host permissions, and real fixtures
are platform-dependent:

| Capability | Required for every bundle | Required when enabled in CI |
| --- | ---: | ---: |
| App bundle and native worker | Yes | Yes |
| CLAP worker handshake/process/recovery | Yes | Yes |
| AU real-device smoke | No | Yes on macOS AU jobs |
| VST3 SDK worker/E2E | No | Yes when `HIRARI_VST3_SDK` and fixture are present |

The build target `hirari-verify-release` is the strict entry point. It builds
the app, builds the CLAP fixture, runs the full test gate, and then verifies
the bundle. Architecture can be overridden with `-DHIRARI_RELEASE_ARCH=...` for
cross-builds; the bundle verifier remains the authority for the actual binary
architecture.

## Legacy asset policy

Files outside the canonical paths above must not be silently restored or
deleted by a feature change. A legacy asset is removable only when one of the
following is true:

- the replacement path is listed in this document;
- a release script no longer references it; and
- the removal is intentional and recorded in the change description.

This keeps the large Rust/Slint migration auditable without treating old
artifacts as production inputs.

## Rust convergence policy

The intended end state is Rust-first with no parallel first-party C++ feature
implementations. Rust owns project state, editing, file formats, validation,
analysis jobs, plugin lifecycle, and UI/application orchestration. C++ is
transitional: each migrated slice gets one Rust implementation, its callers
move to that implementation, and the old C++ path is removed in the same
slice. Do not add matching behavior to both languages to ease a migration.

The long-term target is to remove authored C++ from the production engine as
well. Until the relevant Rust platform/SDK binding replaces them, unavoidable
ABI or operating-system glue must stay a narrow forwarding layer with no
project rules, DSP policy, serialization, or duplicate state. Real-time DSP
and drivers are migration work too; they are not exempt, but move only when
their audio-thread allocation, latency, and device behavior remain within the
existing contracts.

Migration order is based on ownership and duplicate cost, not line count:

1. **Media-decoding implementation migrated.** Project imports and legacy WAV
   utility callers now enter the Rust Symphonia decoder through one C ABI and
   copy interleaved PCM into the engine's existing `AudioBuffer`. The C++ WAV
   parser and FFmpeg subprocess/temp-WAV decoder are gone; the remaining
   `WavDecoder`/`AudioDecoderManager` types preserve caller compatibility,
   import status/cache behavior, and `AudioBuffer` ownership without decoding
   media themselves. Preview decoding shares the same channel-preserving Rust
   decode path. Build validation passed; format fixtures and host/plugin
   playback verification remain outstanding. The bounded WAVE64 float32
   parser also runs in Rust; C++ retains only the compatibility API and copies
   Rust-owned decoded samples into its legacy channel-vector result.
   Native project snapshots, plug-in cache records, memory-integrity checks,
   and asset deltas now call the same Rust CRC32 implementation; their
   independent hand-written C++ checksum loops are removed.
   Native project snapshot file publication also goes through Rust's checked,
   locked atomic-save path. C++ still builds the existing binary snapshot
   payload; that serializer is a remaining migration boundary.
   Plug-in file and bundle content fingerprinting and the binary cache codec
   now run in Rust. Cache reads validate bounded lengths, framing, and trailing
   bytes before returning owned records; C++ still applies plug-in admission
   policy and compares current fingerprints. Cache writes use the same Rust
   encoder and checked atomic-save path. The cache schema/path advanced to v8
   so old native fingerprints are discarded and rebuilt instead of being
   interpreted under the new implementation.
   The bounded-memory RIFF/RF64 stream writers for float32, PCM16, and PCM24
   used by recording and export now run in Rust, including sample conversion,
   interleaving, RF64/Broadcast Wave headers, frame-count validation, file
   sync, atomic rename, and publication locking. Their existing C++ classes
   are compatibility façades. The bounded-memory WAVE64 float32 stream writer
   now follows the same Rust path and remains bounded to 65,536 frames per FFI
   block. Legacy float, PCM16, PCM24, and WAVE64 one-shot C++ entry points now
   dispatch to those same Rust stream writers in bounded chunks, including
   multichannel export up to 32 channels. `WavWriter` is now a compatibility
   façade; it no longer assembles WAVE headers or sample payloads in C++.
   Region waveform peak reduction and cached-resolution selection now run in
   Rust for both the immediate UI envelope and cached reads/background builds.
   Rust now also writes, syncs, memory maps, and removes the temporary cache
   file. C++ retains region lookup, async task scheduling, cache keys, and
   generation-checked publication. The production random-access RIFF/RF64 WAVE
   reader now also keeps its `File` and read-only mapping in Rust, where chunk
   validation and PCM16/24/32 plus float32 decoding run; C++ retains only the
   `MMapAudioFile` compatibility handle used by audio sources and sampler code.
   The `AudioPool` 256/4096-frame peak hierarchy is now generated in Rust from
   that mapping in one deterministic scan; C++ keeps background scheduling,
   cache ownership, and publication to the existing waveform consumer. This
   also removes nested tasks that could occupy every worker while waiting for
   more work from the same pool.
   Realtime MIDI MPE note-state updates and SysEx framing/manufacturer-ID
   validation run in the Rust bridge. The active audio graph's MIDI wrapper is
   now only an opaque Rust-state handle; its C++ MPE policy and the unused
   SysEx queue/articulation-map storage have been removed. Rust also owns the
   articulation transformation routine for callers that supply maps.
   The mastering audio path's FFT, spectral-band analysis, and overlap-add
   processing now run in Rust with persistent preallocated workspaces.
   Spectral matching gains and DDP metadata/image/checksum output also run in
   Rust. Core planar-buffer clearing, sanitization, mixing, gain, stereo copy,
   and magnitude scans now use allocation-free Rust routines. Aligned storage
   allocation and ownership now also live in Rust. C++ retains external channel
   wrapping, pointer views, and
   the profile-lock adapter. The cross-thread parameter smoother's atomic state,
   coefficient setup, block ramp, and next-value update now live in Rust; its
   C++ class is a compatibility façade. The scalar parameter registry's map,
   range checks, and locking also moved to Rust; the unused C++ managed-parameter
   duplicate was removed. The active sampler's five-stage AHDSR state machine,
   timing counters, sample-rate progression, and MIDI 1.0/2.0 channel-voice
   decoding now also run in Rust; its C++ wrapper forwards event and audio
   buffer views. Events retain the existing block-level application timing. The
   shared AtomicParameter state, smoothing curves, block ramps, normalization,
   and display formatting have moved to Rust as well. The active channel-strip
   gain/pan sample loop, mirror-tap handling, and mute clearing now run in Rust;
   C++ keeps only the AudioBuffer pointer adapter and public class surface.
   MidiBuffer's bounded event writes, fixed-layout copies, and stable
   sample-offset/priority ordering are now implemented in Rust against the
   shared event array; C++ retains overflow telemetry and fragmented-message
   reassembly. Its unused standalone ingestion dispatcher was retired. Fade
   crossfades, micro-fades, and linear,
   equal-power, ease, and Bezier gain curves now share the allocation-free Rust
   implementation; the C++ source only forwards the legacy API. The active
   mastering True Peak Limiter's intersample detector, gain envelope, delayed
   sample ring, reset, latency, and tail calculations now live in Rust; its
   workspaces allocate only during prepare.
   The unreferenced native Alchemy sampler implementation has been retired;
   its multisample Classic, Granular, Additive, and Spectral render modes now
   live in the Rust core with owned sample zones and fixed realtime workspaces.
   Collaboration users, queued edits, immutable commit history, and two-parent
   LWW merges now share the Rust cloud model. The detached C++ orchestrator and
   its non-cryptographic 64-bit commit hash have been removed; Rust validates
   imported commit content with SHA-256.
   AnalysisHub's log-spectrum remapping, phase heatmap, local-peak extraction,
   mel-band projection, motion vectors/energy, synesthesia RGB mapping,
   mastering advice rules, and dashboard/song-structure JSON formatting now
   execute in `hirari-core-bridge/src/analysis_views.rs`. Its active C++ caller
   snapshots engine telemetry and forwards borrowed slices through CXX; it no
   longer computes those views or depends on `EngineAnalyzer` for its fixed
   advice rules. NeuralBridge's EBU loudness/peak advice policy and fixed
   lock-free SPSC queue now also run in Rust; C++ supplies the monotonic clock
   value and retains a forwarding façade. Structural section sizing,
   classification, motivic IDs, narrative-flow scoring, and energy smoothing
   now run in Rust for both the song-structure view and automatic arrangement;
   the duplicate native `NeuralArrangementKernel` was removed. C++ still
   snapshots engine regions and harmonic context. The AnalysisHub façade,
   loudness-history lock/storage, and engine telemetry sampling remain native.
   `AudioEngine`'s runtime-health vector aggregation and captured planar-input
   to interleaved-buffer conversion now run in Rust (`runtime_views.rs`). C++
   still owns the audio-device poll, telemetry snapshot, and its bounded scratch
   storage; only the platform data collection remains at that boundary.
   The fixed-capacity SPSC queue between device callbacks and recording/input
   polling now owns its slots and atomic indices in Rust (`audio_input_queue.rs`).
   C++ keeps only the queue handle and the OS callback boundary; sample copies,
   queue publication, polling, and drop accounting are Rust code.
   The active engine tempo-map's sample-to-beat and beat-to-sample conversion,
   including ramp integration and bounded inverse solving, now execute in Rust
   over the existing immutable C++ event snapshot. Rust also sorts and validates
   beat-positioned edits, assigns their sample positions, and recalculates
   integrated beat positions after sample-position edits. Adding, replacing,
   and removing sample-position events, including ordering and recalculation,
   now also run in Rust. Sample-position BPM lookup and time-signature
   validation, lookup, sample mapping, and edit operations also run in Rust.
   C++ retains event locking/snapshot publication; realtime tempo queries
   allocate nothing.
   The active routing engine's deterministic topological compilation now runs
   in Rust (`routing_graph_pdc.rs`) over the live audio and processing edges.
   Rust validates node bounds, detects cycles both during connection edits and
   graph compilation, computes indegrees, and returns the ascending-node
   execution order. C++ snapshots the edges on the control thread and atomically
   publishes that order for realtime readers. Send-path PDC delay history,
   requested-delay atomics, finite-sample handling, and the 64-sample delay
   transition now also run in Rust without audio-thread allocation. C++ retains
   only opaque-state ownership and deferred reclamation after block readers
   exit. The track/bus/send PDC graph solver now uses Rust's canonical routing
   schedule for path latency, fan-in edge compensation, cycle rejection, and
   global output latency; C++ retains control inputs and double-buffered atomic
   publication. The shared `PDCGraphSolver` C++ class is now an ABI adapter to
   that same Rust calculation, so `LatencyManager` and the staged routing graph
   no longer keep a second path-latency implementation. The staged graph's
   topological execution order and longest-path stage assignment also run in
   Rust; C++ keeps only its node containers and stage-facing API.
   The active audio callback's stereo and planar routing fanout loops now also
   run in allocation-free Rust kernels. Rust owns normal/Send gain and
   pre-fader atomic tables and reads them directly during fanout. C++ retains
   editable graph-edge snapshots and topology publication. Fixed-capacity
   feedback-edge state and its 8192-frame stereo buffers now live in Rust too;
   control edits, undo snapshots, capture/injection, and gain lookup cross a
   narrow opaque handle. Feedback samples use atomics to avoid unsynchronized
   shared mutable buffers across the control and audio threads.
   The orphan native `NotationOrchestrator` (no production or test callers)
   has been retired after its timeline layout, collision spacing, note stems,
   short-note beams, non-overlap slurs, and primitive generation were moved
   into the Rust notation orchestrator. Its previous Rust placeholder that
   positioned notes by insertion index is replaced with time-based layout.
   The separate, unused C++ `AssetLibraryEngine` declaration and implementation
   have also been removed. Production asset catalog and sample loading already
   use `hirari-core-bridge/src/modules/asset_library.rs`; this removes a second,
   unreachable WAV-only indexer with no callers.
   The active region time-stretch path now executes Signalsmith's modified
   real-FFT forward and inverse transforms in Rust through preallocated plans.
   Its Kaiser perfect-reconstruction window is also generated and normalized in
   Rust during preparation; the C++ runtime skips its duplicate normalization.
   The stretch algorithm and surrounding STFT scheduling remain C++ for now.
   C++ and Rust transform/window outputs are compared at multiple sizes, and
   streaming/seek differential checks pass through the Rust-backed production
   kernel. This migrates inner DSP kernels, not the full time-stretch engine.
2. Move control-plane project and edit operations whose canonical state is
   already `ProjectDocument` into Rust, replacing native JSON round trips and
   rollback snapshots as each transaction becomes fully Rust-owned.
3. Port the remaining engine/domain modules in dependency order, then replace
   platform audio/MIDI and plugin ABI shims with Rust bindings or generated
   bindings. Keep one implementation per behavior throughout.

For each slice, record its current C++ owner, Rust destination, active callers,
FFI removed, and remaining compatibility surface. A slice counts as migrated
only when production callers use Rust and the old C++ implementation is no
longer compiled or reachable. Existing FFI boolean/empty-string contracts,
WAV read/write duplication, and native-generation gaps remain migration work;
they are not reasons to introduce another parallel implementation.
