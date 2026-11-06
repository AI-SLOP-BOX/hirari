#pragma once

// `size_t` is used by the limits below, so the header that defines it has to
// come first. This include used to sit *after* the namespace, which compiled
// only as long as every includer happened to pull in <cstddef> transitively;
// the spectral-editor and mmap-audio contracts reached rust_ffi.hpp first and
// failed on `unknown type name 'size_t'`.
#include <cstddef>
#include <cstdint>
#include <string_view>
#include <type_traits>


// Physical limits shared by the C++ engine and the Rust core. These mirror
// `crate::dsp_limits` exactly; the two sides have to agree because a clamp on
// one side and a rejection on the other is how a caller ends up storing a
// value the other side would have refused.
namespace hirari::limits {
/// Cents per octave, the denominator of every cents-to-ratio conversion.
constexpr double kCentsPerOctave = 1200.0;
/// Lowest sample rate any module accepts, in Hz.
constexpr double kMinSampleRate = 8000.0;
/// Highest sample rate any module accepts, in Hz.
constexpr double kMaxSampleRate = 384000.0;
/// Upper bound on the entries a per-region edit table may hold.
constexpr size_t kMaxEditEntries = 4096;
/// Maximum sample span accepted by the Rust audio quantizer.
constexpr uint64_t kMaxAudioQuantizerWarpSamples = 16'777'216u;
} // namespace hirari::limits

extern "C" const void* hirari_log_buffer_default_state();
extern "C" void hirari_log_buffer_post(
    const void* state, uint32_t level, uint32_t component_id,
    const uint8_t* message, size_t message_len);

// The diagnostic ring is Rust-owned. C++ sites post and address it through
// these two helpers instead of carrying their own wrapper class.
inline void hirari_log_post(uint32_t level, uint32_t component_id,
                            std::string_view message) noexcept {
    hirari_log_buffer_post(hirari_log_buffer_default_state(), level, component_id,
                           reinterpret_cast<const uint8_t*>(message.data()), message.size());
}
inline void* hirari_log_state() noexcept {
    return const_cast<void*>(hirari_log_buffer_default_state());
}

// The engine's command and event queues are Rust-owned fixed-capacity SPSC
// rings (engine_rings.rs). C++ keeps neither the storage nor the index
// arithmetic: it holds an opaque per-engine handle, and the audio callback
// drains what the Rust command host admitted. Field order and width here match

extern "C" void hirari_engine_command_ring_clear(const void* rings);
extern "C" bool hirari_engine_event_ring_push(const void* rings, uint64_t timestamp,
                                              uint32_t type, uint32_t track_id, float value,
                                              const uint8_t* label, size_t label_len);

// Engine track table (engine_track_table.rs): the live document graph, its
// deferred-destruction list and the double-buffered snapshot the realtime
// callback traverses. The tracks themselves are Rust objects; the table owns
// their Arcs and publishes their object pointers.
// The published graph.
struct HirariTrackSnapshot {
    void* const* active_tracks;
    void* const* track_map;
    uint32_t track_count;
    uint64_t audio_config_generation;
};
extern "C" size_t hirari_engine_track_count(void* table);
extern "C" void* const* hirari_engine_track_ptrs(void* table);
extern "C" void* hirari_engine_track_find(void* table, uint32_t track_id);
extern "C" void* hirari_engine_track_retain(void* table, uint32_t track_id);
extern "C" bool hirari_engine_track_insert(void* table, size_t index, void* owner);
extern "C" void hirari_engine_track_defer_all(void* table);
extern "C" void hirari_engine_track_publish(void* table, uint64_t generation, bool stamp);
extern "C" void hirari_engine_track_snapshot(void* table, HirariTrackSnapshot* out);
// The ordered control walk over the graph: bus reconcile, routing rebuild,
// snapshot publication, then delay compensation. See engine_track_table.rs.
extern "C" void hirari_engine_sync_structure(void* table, void* bus_system, void* routing,
                                             void* pdc, uint64_t generation);


extern "C" void hirari_input_blocks_reset(void* blocks);
extern "C" void hirari_input_blocks_publish_monitor(void* blocks, const float* const* inputs,
                                                    uint32_t channel_count, uint32_t frames);
extern "C" void hirari_input_blocks_end_monitor(void* blocks);
extern "C" void hirari_input_blocks_publish_talkback(void* blocks, const float* const* inputs,
                                                     uint32_t channel_count, uint32_t frames,
                                                     const void* control_room);
extern "C" void hirari_input_blocks_end_talkback(void* blocks);


extern "C" void hirari_external_sync_set_rate(void* state, double bpm);
extern "C" double hirari_external_sync_rate(const void* state);
extern "C" void hirari_external_sync_tick(void* state, uint64_t timestamp);
extern "C" uint64_t hirari_external_sync_ticks(const void* state);
extern "C" uint64_t hirari_external_sync_last_tick(const void* state);
extern "C" void hirari_master_limiter_prepare(void* state, double sample_rate);
extern "C" void hirari_master_limiter_reset(void* state);
extern "C" void hirari_control_room_reset(void* state);
extern "C" void hirari_bus_system_reset(void* state);
extern "C" void* hirari_audio_buffer_storage_create();
extern "C" void hirari_audio_buffer_storage_destroy(void* storage);
extern "C" uint8_t hirari_audio_buffer_storage_reserve_buffer(
    void* storage, uint32_t channels, uint32_t samples);
extern "C" bool hirari_audio_buffer_storage_resize(
    void* storage, uint32_t channels, uint32_t samples);
extern "C" bool hirari_audio_buffer_storage_wrap_channels(
    void* storage, float** channels, uint32_t channel_count, uint32_t sample_count);

extern "C" void hirari_audio_buffer_storage_clear(
    const void* storage, uint32_t offset, uint32_t samples);







// Rust-owned per-region processing-step history. `operation` is borrowed for
// the duration of a call and copied into the store; nothing on the render path
// reads it, so no published projection exists.
struct HirariEventStep {
    uint32_t id = 0;
    const uint8_t* operation = nullptr;
    size_t operation_len = 0;
    float parameter = 0.0f;
    bool enabled = true;
};

// Region edit host bridge.
//
// The region table lives in Rust (region_store.rs); the engine keeps only the
// handles the store cannot own — decoded audio buffers and the publish target.
// Rust owns the edit policy: it reads the current state, decides, applies and
// records exactly one undo entry, all inside the store. Every callback runs on
// the control thread; none of them is reachable from the audio callback.
//


// Split/duplicate policy is Rust-owned, including the note-table split and
// the undo capture. C++ keeps the payload (buffers, cache, note-store handle)
// and clones/stores it; these calls never touch the audio thread.
//
// Splits a region at `splitBeat` (an absolute timeline sample position) and
// returns the new right-hand id, or 0.
// Duplicates a region at `startSample` and returns the new id, or 0.

// Bounce planning is Rust-owned end to end: the driver walks the region, MIDI,
// device-format and limiter stores it already has handles for and computes the
// render extent itself (`hirari-core-bridge/src/bounce_plan.rs`,
// `bounce_driver.rs`). C++ contributes only the store handles below.
enum HirariBounceStore : uint32_t {
    kHirariBounceStoreTrackTable = 0,
    kHirariBounceStoreEngineBlock = 1,
    kHirariBounceStoreMidiSnapshot = 2,
    kHirariBounceStoreMasterLimiter = 3,
};

// Host operations for the Rust bounce driver. Only the audio graph stays here:
// the render block, the transport, the reproducible-reset pass and the handles
// of the stores the Rust plan walk reads. Everything else a bounce owns - the
// plan, offline-render ownership, progress, the lifecycle state, cancel/pause,
// the async worker and the publish boundary - is Rust state (bounce_driver.rs)
// below. Every entry must be set: a missing operation fails the render.
struct HirariBounceHostOps {
    uint8_t (*run)(void* host, const uint8_t* path, size_t path_length, uint32_t format);
    void (*reset)(void* host);
    void* (*store)(void* host, uint32_t kind);
    void (*render)(void* host, float* left, float* right, uint32_t frames);
    uint32_t (*render_chunk)(void* host);
    void (*read_transport)(void* host, uint64_t* playhead, uint8_t* playing);
    void (*set_transport)(void* host, uint64_t playhead, uint8_t playing);
};
static_assert(sizeof(HirariBounceHostOps) == 7 * sizeof(void*));

extern "C" bool hirari_bounce_render_driver(
    void* host, void* session, const HirariBounceHostOps* ops, const uint8_t* path,
    size_t path_length, uint32_t format);
extern "C" bool hirari_bounce_start_async(
    void* session, void* host, const HirariBounceHostOps* ops, const uint8_t* path,
    size_t path_length, uint32_t format);
extern "C" void hirari_bounce_join(void* session);
extern "C" bool hirari_bounce_cancel(void* session), hirari_bounce_pause(void* session),
    hirari_bounce_resume(void* session);
extern "C" uint32_t hirari_bounce_state(void* session);
extern "C" float hirari_bounce_progress(void* session);



// The lifetime handle of an imported Rust audio buffer is Rust's own
// (storage_handle.rs): the region store views its planes and releases the
// handle through these entry points.
extern "C" uint32_t hirari_storage_handle_view(void* audio, const float** out,
                                               size_t capacity, uint64_t* samples);
extern "C" void hirari_storage_handle_release(void* audio);

struct HirariRegionHostOps {
    void* findRegion = nullptr;
    void* applyEdit = nullptr;
    void* listSyncGroup = nullptr;
    void* recordUndo = nullptr;
    void* noteEdit = nullptr;
    void* snapshotRegion = nullptr;
    void* releaseSnapshot = nullptr;
    void* warpMarkers = nullptr;
    void* rangeEdits = nullptr;
    void* retainAudio = nullptr;
    uint32_t (*audioView)(void* handle, const float** out, size_t capacity,
                          uint64_t* samples) = nullptr;
    void (*releaseAudio)(void* handle) = nullptr;
    void* trackToken = nullptr;
    size_t (*listTracks)(void* host, uint32_t* out, size_t capacity) = nullptr;
    size_t (*listRegions)(void* host, uint32_t trackId, uint32_t* out,
                          size_t capacity) = nullptr;
    void* (*tempoState)(void* host) = nullptr;
    void* (*undoState)(void* host) = nullptr;
    double (*sampleRate)(void* host) = nullptr;
    void* regionPath = nullptr;
    void* insertRegion = nullptr;
    size_t (*trackIdentity)(void* host, uint32_t trackId) = nullptr;
    void* (*retainTrack)(void* host, uint32_t trackId) = nullptr;
    void (*releaseTrack)(void* handle) = nullptr;
    bool (*pitchBegin)(void* trackHandle) = nullptr;
    void (*pitchFinish)(void* trackHandle) = nullptr;
    void* (*noteSnapshot)(void* trackHandle, uint32_t regionId) = nullptr;
    void* cloneRegion = nullptr;
    void* compRanges = nullptr;
};
static_assert(sizeof(HirariRegionHostOps) == 28 * sizeof(void*));

extern "C" bool hirari_engine_auto_arrange(
    void* host, float bpm, double sample_rate, float tension, float valence,
    void (*emitSection)(void*, uint64_t, uint64_t));

extern "C" void hirari_region_host_install(const HirariRegionHostOps* ops);
extern "C" int32_t hirari_region_import_add(
    void* host, uint32_t trackId, const uint8_t* path, size_t pathLength,
    double startSample);
extern "C" void hirari_region_jobs_clear(void* host);
extern "C" void hirari_region_jobs_shutdown();
extern "C" size_t hirari_float_buffer_len(const void* buffer);
extern "C" const float* hirari_float_buffer_data(const void* buffer);
extern "C" void hirari_float_buffer_destroy(void* buffer);
// Environment-driven device admission (audio_device_mode.rs).
extern "C" bool hirari_native_test_isolation();
// Audio configuration bounds (audio_config_rules.rs).
extern "C" bool hirari_audio_format_valid(double sampleRate, uint32_t blockSize);
extern "C" void* hirari_region_waveform(
    void* host, uint32_t trackId, uint32_t regionId, uint32_t numPeaks);
extern "C" void hirari_region_waveform_clear(void* host);
// Decodes `path`, swaps it into the region and records the undo pair. Rust
// owns the path/rate policy and the snapshots; C++ performs the swap.


// The region table and the host callbacks it reads are Rust-owned
// (region_store.rs, engine_track.rs). The track installs them itself; this must
// run before the region edit host installs, because the edit host only routes
// its region-data callbacks once the storage host already answers.
extern "C" void hirari_track_region_host_install();
extern "C" void hirari_region_store_trim_retired(void* track);
extern "C" size_t hirari_region_store_list_regions(void* host, uint32_t trackId,
                                                   uint32_t* out, size_t capacity);
extern "C" void* hirari_region_store_note_snapshot(void* track, uint32_t regionId);
extern "C" bool hirari_region_store_append_processing(void* track, uint32_t regionId,
                                                      HirariEventStep step);
extern "C" bool hirari_region_store_clear_processing(void* track, uint32_t regionId);
extern "C" void hirari_master_meter_prepare(void* state, double sample_rate);

extern "C" void hirari_mpe_state_reset(void* state);
extern "C" float hirari_tempo_map_get_current_bpm(const void* state);
extern "C" void hirari_tempo_map_set_current_bpm(void* state, float bpm);
// The engine's set_tempo entry point, implemented in Rust like every other
// tempo edit and reached only from here because a native contract test drives
// that method straight off the engine singleton.
extern "C" bool hirari_tempo_edit_set_tempo(void* tempo, void* undo, const void* log,
                                             uint64_t playhead, double sample_rate, float bpm);
// The graph, gain, feedback, send-PDC and topology stores are reached only
// through hirari_routing_engine_* (routing_engine_core.rs). The block walk and
// its fanout descriptors live in Rust (engine_block_host.rs).

// Link registration is a control-thread edit owned by Rust (routing_edit_host.rs
// taps the source's work buffer through the track session); C++ no longer calls
// either entry point.
extern "C" bool hirari_sidechain_engine_has_link(
    const void* state, uint32_t destination_track, uint32_t plugin_index, uint32_t source_track);
extern "C" void hirari_sidechain_engine_reset(void* state);
extern "C" void hirari_sidechain_routes_clear(const void* state);
extern "C" void hirari_routing_engine_reset(void* state);

// Routing edit host bridge.
//
// Rust owns validation, cycle/feedback rejection, send-vs-insert resolution
// and the undo capture; the engine owns the live routing objects and applies
// the resolved mutations under its control locks. Keep this layout in sync
// with hirari-core-bridge/src/routing_edit_host.rs.



struct HirariRoutingHostOps {
    void* (*routingStore)(void*) = nullptr;
    void* (*sidechainRoutesStore)(void*) = nullptr;
    void* (*sidechainEngineStore)(void*) = nullptr;
    void* sidechainContext = nullptr;
    void* registerSidechain = nullptr;
    void (*markLayoutChanged)(void*) = nullptr;
    void (*syncStructural)(void*) = nullptr;
    void* recordUndo = nullptr;
    void* (*undoState)(void*) = nullptr;
    void* (*engineBlockStore)(void*) = nullptr;
};
static_assert(sizeof(HirariRoutingHostOps) == 10 * sizeof(void*));

extern "C" void hirari_routing_edit_host_install(const HirariRoutingHostOps* ops);

// Track lifecycle edit policy (create, duplicate, remove and their undo). The
// graph walk, the captures and the replay are Rust's (track_edit_host.rs): it
// takes the tracks lock, builds the Rust track, moves the Rust track table,
// publishes the audio snapshot and restores the sidechain links, routes and
// routing connections. C++ contributes only the handles of the stores the walk
// visits.
enum HirariTrackEditStore : uint32_t {
    kHirariTrackEditStoreTrackTable = 0,
    kHirariTrackEditStoreEngineBlock = 1,
    kHirariTrackEditStoreSidechain = 2,
    kHirariTrackEditStoreSidechainRoutes = 3,
    kHirariTrackEditStoreRouting = 4,
    kHirariTrackEditStoreLayoutRevision = 5,
    kHirariTrackEditStoreBusSystem = 6,
};

struct HirariTrackEditHostOps {
    // Lists track ids / one track's region ids in storage order. The Rust id
    // allocator reads the project through these before it hands out an id.
    size_t (*listTracks)(void*, uint32_t*, size_t) = nullptr;
    size_t (*listRegions)(void*, uint32_t, uint32_t*, size_t) = nullptr;
    void* (*store)(void*, uint32_t) = nullptr;
    using ReplayFn = void (*)(void*);
    void (*recordReplay)(void*, const uint8_t*, size_t, void*, ReplayFn, ReplayFn,
                         ReplayFn) = nullptr;
    void (*syncStructuralChanges)(void*) = nullptr;
    void* (*undoState)(void*) = nullptr;
};

extern "C" void hirari_track_edit_host_install(const HirariTrackEditHostOps* ops);

// Track and region id allocation lives in Rust (id_host.rs). An emptied project
// restarts the counters; a loaded document seeds them from Rust. Store-only and
// safe under the track lock.
extern "C" void hirari_ids_reset_project(void* host);

// Track freeze (track_freeze_host.rs). The Rust side walks the track's own
// runtime block by block, writes and reads the Float32 artifact and publishes
// the snapshot; the engine only supplies what cannot leave C++.
struct HirariTrackFreezeHostOps {
    // Pins a track; `wait` drains the audio callback before lookup. The
    // returned handle must be released.
    void* (*acquire)(void*, uint32_t, bool) = nullptr;
    void (*release)(void*) = nullptr;
    uint32_t (*blockSize)(void*) = nullptr;
};

extern "C" void hirari_track_freeze_host_install(const HirariTrackFreezeHostOps* ops);

// Control-command admission is Rust's; the queue itself is a Rust ring the
// engine's session bag holds, so the engine only forwards the push.
extern "C" bool hirari_engine_push_command(void* host, uint32_t type, uint32_t trackId,
                                           float value, uint64_t timestamp,
                                           uint64_t expectedProjectGeneration,
                                           uint64_t expectedAudioGeneration);

// Track control policy (spatial validation, HRTF admission, auto-mix curve)
// is Rust's; it pins a track through its own session over the engine's graph.
extern "C" bool hirari_plugin_edit_set_bypass(
    void* host, uint32_t trackId, uint32_t pluginIndex, bool bypassed);

// Status pointers refer to the Rust sandbox status that engine_track.rs fills
// and plugin_sandbox_maintenance.rs reads; C++ only forwards the visitor.
using HirariPluginSandboxVisitorFn = bool (*)(void* context, uint32_t trackId, const void* status);
struct HirariPluginSandboxHostOps {
    // Reports every track's statuses in engine order.
    bool (*statuses)(void* host, void* context, HirariPluginSandboxVisitorFn visit) = nullptr;
    // Restarts one track's rejected sandboxes, then reports the resulting
    // statuses; returns how many were recovered.
    uint32_t (*maintain)(void* host, uint32_t trackId, bool autoRestart, void* context,
                         HirariPluginSandboxVisitorFn visit) = nullptr;
    void (*publishEvent)(void* host, uint32_t type, uint32_t trackId, float value,
                         const uint8_t* label, size_t labelLength) = nullptr;
};
extern "C" void hirari_plugin_sandbox_maintenance_host_install(
    const HirariPluginSandboxHostOps* ops);
extern "C" void hirari_plugin_sandbox_maintenance_forget(void* host);
// Periodic sandbox recovery loop and journal start are Rust-owned
// (sandbox_recovery_runtime.rs); the engine only lends its host pointer.
extern "C" void hirari_sandbox_recovery_start(void* host, void* journal);
extern "C" void hirari_sandbox_recovery_stop();

// The Track object is Rust's (engine_track.rs): one Arc<Track> owning the
// channel strip, effect chain, automation, runtime, panner and region table.
// C++ holds two opaque pointers — the object pointer, which addresses the
// behavior entry points below and keys the region store, and the owning handle,
// which is the lifetime currency the track table and the host callbacks pass.
enum HirariTrackKind : uint32_t {
    kHirariTrackKindAudio = 0,
    kHirariTrackKindMidi = 1,
    kHirariTrackKindInstrument = 2,
    kHirariTrackKindBus = 3,
    kHirariTrackKindVocal = 4,
};
// The Rust sub-stores reachable through hirari_track_storage.
enum HirariTrackStore : uint32_t {
    kHirariTrackStoreStrip = 0,
    kHirariTrackStoreChain = 1,
    kHirariTrackStoreAutomation = 2,
    kHirariTrackStoreFrozen = 3,
    kHirariTrackStoreRuntime = 4,
    kHirariTrackStoreArticulation = 5,
    kHirariTrackStoreName = 6,
    kHirariTrackStoreWorkBuffer = 7,
    kHirariTrackStorePluginMidi = 8,
    kHirariTrackStoreBusState = 9,
    kHirariTrackStoreSendPre = 10,
    kHirariTrackStoreSendPost = 11,
};
// Bumping this revision is the only signal the layout gather observes.
extern "C" void hirari_layout_revision_mark(void* state);
// Builds one track and returns its owning handle; `out_track` receives the
// object pointer. The caller gives the handle to the track table.
extern "C" void* hirari_track_new(uint32_t id, const uint8_t* name, size_t name_len,
                                  uint32_t kind, void* revision, void** out_track);
extern "C" void hirari_track_owner_release(void* owner);
extern "C" void* hirari_track_owner_object(void* owner);
extern "C" uint32_t hirari_track_id(void* track);
extern "C" uint32_t hirari_track_type(void* track);
extern "C" void* hirari_track_storage(void* track, uint32_t store);
extern "C" void hirari_track_reset_for_offline_render(void* track);
extern "C" bool hirari_track_pitch_begin(void* track);
extern "C" void hirari_track_pitch_finish(void* track);
extern "C" bool hirari_track_expression_map_set(void* track, uint8_t kind,
                                                const uint8_t* data, size_t length);
extern "C" bool hirari_track_visit_sandbox_statuses(void* track, void* context,
                                                    HirariPluginSandboxVisitorFn visit);
extern "C" uint32_t hirari_track_maintain_sandboxes(void* track, bool autoRestart,
                                                    void* context,
                                                    HirariPluginSandboxVisitorFn visit);

extern "C" void hirari_native_pdc_reset_for_project(const void* state);




/// Rust owns the engine's whole block (engine_block_host.rs): the admission
/// head, the graph walk and the post-graph tail. The engine binds its handles
/// once, then calls begin, route and finish around the device buffer. The block
/// also holds the session scalars - device format, master fader, record-arm
/// gate, shutdown flag, in-flight count, control token, project tonality - so
/// the engine keeps no atomics of its own.
///
/// The block is one slot of a Rust bag (engine_session.rs) that holds every
/// store the engine renders and edits with, and owns their creation and
/// teardown order. The engine keeps the bag and asks it for a handle by kind.
enum HirariEngineHandleKind : uint32_t {
    kHirariHandleTrackTable = 0,
    kHirariHandleInputBlocks = 1,
    kHirariHandleMetronome = 2,
    kHirariHandleEngineRings = 3,
    kHirariHandleMidiPlaybackClock = 4,
    kHirariHandleMpeState = 5,
    kHirariHandlePreviewSynth = 6,
    kHirariHandlePreviewSampler = 7,
    kHirariHandleMeter = 8,
    kHirariHandleMasterLimiter = 9,
    kHirariHandleEngineBlock = 10,
    kHirariHandleMixerTelemetry = 11,
    kHirariHandleBounceSession = 12,
    kHirariHandleSidechainRoutes = 13,
    kHirariHandleMidiSnapshotStore = 14,
    kHirariHandlePdc = 15,
    kHirariHandleUndo = 16,
    kHirariHandleTempo = 17,
    kHirariHandleMpeManager = 18,
    kHirariHandleRouting = 19,
    kHirariHandleSidechainEngine = 20,
    kHirariHandleBusSystem = 21,
    kHirariHandleMacroControl = 22,
    kHirariHandleMidiLearn = 23,
    kHirariHandleForensicJournal = 24,
    kHirariHandleControlRoom = 25,
    kHirariHandleExternalSync = 26,
    kHirariHandleArrangementDocument = 27,
    kHirariHandleLayoutRevision = 28,
};
extern "C" void* hirari_engine_session_create(void* host, void* tracks_mutex);
/// The handle of one slot, or null for an unknown kind and a retired slot.
extern "C" void* hirari_engine_session_handle(const void* state, uint32_t kind);
/// Retires the track table and the hardware input store after the engine has
/// joined its callbacks. Idempotent because both slots go null with them.
extern "C" void hirari_engine_session_close_graph(void* state);
/// Releases the whole bag, the block before the stores it borrows.
extern "C" void hirari_engine_session_destroy(void* state);
/// Zero when the head already silenced the window. `frames_out` receives the
/// block length the rest of the callback must use.
extern "C" uint8_t hirari_engine_block_begin(
    void* state, void* output, uint32_t off, uint32_t frames, uint32_t* frames_out);
extern "C" uint64_t hirari_engine_block_chunk_plan(void* state, uint32_t remaining);
extern "C" void hirari_engine_block_route(
    void* state, void* output, uint32_t off, uint32_t frames);
extern "C" void hirari_engine_block_finish(
    void* state, void* output, uint32_t off, uint32_t frames);
extern "C" uint64_t hirari_engine_block_playhead(const void* state);
extern "C" void hirari_engine_block_set_playhead(void* state, uint64_t playhead);
extern "C" uint8_t hirari_engine_block_is_playing(const void* state);
extern "C" void hirari_engine_block_set_playing(void* state, uint8_t playing);
extern "C" void hirari_engine_block_set_test_tone(void* state, uint8_t enabled);
extern "C" uint64_t hirari_engine_block_generation(const void* state);
extern "C" uint64_t hirari_engine_block_bump_generation(void* state);
extern "C" double hirari_engine_block_device_sample_rate(const void* state);
extern "C" uint32_t hirari_engine_block_device_block_frames(const void* state);
extern "C" void hirari_engine_block_set_device_format(
    void* state, double sample_rate, uint32_t block_frames);
extern "C" void* hirari_engine_block_layout_revision(const void* state);
extern "C" float hirari_engine_block_master_gain(const void* state);
extern "C" void hirari_engine_block_set_master_gain(void* state, float gain);
extern "C" uint8_t hirari_engine_block_is_shutdown(const void* state);
/// Sets the shutdown flag and reports whether it was already set.
extern "C" uint8_t hirari_engine_block_take_shutdown(void* state);
extern "C" void hirari_engine_block_callback_enter(void* state);
extern "C" void hirari_engine_block_callback_leave(void* state);
/// Waits for every callback in flight; a control path, never the realtime one.
extern "C" void hirari_engine_block_join_callbacks(const void* state);
extern "C" uint64_t hirari_engine_block_mark_control(void* state);
extern "C" void hirari_engine_block_set_project_scale(
    void* state, int32_t root, int32_t scale);
/// Prepare every track in the engine's table for the format the block holds:
/// drop a freeze the rate cannot replay, retune and grow the track's blocks,
/// then re-offer its work taps to the sidechain graph. The caller holds the
/// tracks lock. Returns the first allocation status the walk cannot swallow: 2
/// an impossible size, 3 an exhausted allocation, 0 otherwise.
extern "C" uint8_t hirari_engine_block_prepare_tracks(void* state);
/// Prepares one track for the format the block already holds.
extern "C" uint8_t hirari_engine_block_prepare_track(void* state, void* track);
extern "C" void hirari_engine_block_set_config_transition(
    void* state, uint8_t transition);
extern "C" void hirari_engine_block_clear_midi_events(void* state);


extern "C" void hirari_undo_manager_clear(void* state);

// MIDI expression-map edit policy. Rust validates and applies the packed map
// through the track's articulation runtime; the engine only resolves the
// track handle and stores the map JSON.
using HirariExpressionMapTrackArticulationFn = void* (*)(void*, uint32_t);
using HirariExpressionMapSetJsonFn =
    bool (*)(void*, uint32_t, uint8_t, const uint8_t*, size_t);
struct HirariExpressionMapEditHostOps {
    HirariExpressionMapTrackArticulationFn trackArticulation = nullptr;
    HirariExpressionMapSetJsonFn setMapJson = nullptr;
};
extern "C" void hirari_expression_map_edit_host_install(
    const HirariExpressionMapEditHostOps* ops);
// Macro bindings are added by Rust directly against the learn and macro-control
// stores it owns, so no mapping entry point is declared here.

extern "C" void hirari_mpe_manager_reset(void* state);


using HirariSnapshotDestroyCallback = void (*)(void*);
using HirariWaveformComputeCallback = size_t (*)(void*, uint32_t, uint32_t, float*, size_t);
// Retires the Rust layout-snapshot session bound to this engine and joins its
// worker, so a background save can never outlive the graph it reads.
extern "C" void hirari_layout_snapshot_owner_release_host(void* host);
extern "C" uint64_t hirari_waveform_snapshot_request(
    void* context, uint32_t track_id, uint32_t region_id,
    HirariWaveformComputeCallback compute, HirariSnapshotDestroyCallback destroy_context);

// Project-layout gathering. Rust walks the live engine model through the session
// bag this host address resolves to (layout_gather.rs), owns the snapshot and
// its backing blocks, and serializes or saves it in one call. The engine
// contributes its address and the tracks lock the gather brackets with it, so
// no per-field host table crosses here any more.
extern "C" bool hirari_project_save_from_host(
    void* host, const uint8_t* path, size_t path_length);
extern "C" bool hirari_project_load_from_host(
    void* host, const uint8_t* path, size_t path_length);

// Rust-owned harmonic context shared by structure analysis and auto-arrangement.
extern "C" void hirari_harmonic_context_get(
    float* tension, float* valence, float* stability);
