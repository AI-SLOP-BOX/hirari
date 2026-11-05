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
// engine_rings::CommandRecord.
struct HirariEngineCommandRecord {
    uint32_t type;
    uint32_t tid;
    float val;
    uint64_t ts;
    uint64_t expectedAudioGeneration;
};
static_assert(sizeof(HirariEngineCommandRecord) == 32,
              "HirariEngineCommandRecord must match the Rust CommandRecord layout");
extern "C" void hirari_engine_command_ring_clear(const void* rings);
extern "C" bool hirari_engine_event_ring_push(const void* rings, uint64_t timestamp,
                                              uint32_t type, uint32_t track_id, float value,
                                              const uint8_t* label, size_t label_len);
extern "C" bool hirari_engine_event_ring_pop(const void* rings, uint64_t* timestamp,
                                             uint32_t* type, uint32_t* track_id, float* value,
                                             uint8_t* label, size_t label_capacity);

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
// The project-load commit point: the outgoing graph's sidechain entries, every
// project-scoped manager and the graph itself, in the order Rust owns.
extern "C" void hirari_engine_reset_project(void* table, void* rings, void* pdc,
                                            void* sidechain, void* routing, void* bus_system,
                                            void* control_room, void* sidechain_routes);

// Live input staging (engine_input_block.rs): the driver callback copies the
// monitored input pair and the Control Room talkback source into the planar
// buffers Rust owns; the render callback reads one window of a chunk.
struct HirariInputMonitorBlock {
    uint32_t track_id;  // kNoInputMonitorTrack when nothing is monitorable
    float* left;
    float* right;
};
extern "C" bool hirari_input_blocks_set_focus(void* blocks, uint32_t track_id,
                                              uint32_t left_channel, uint32_t right_channel);
extern "C" void hirari_input_blocks_clear_focus(void* blocks, uint32_t track_id);
extern "C" void hirari_input_blocks_reset(void* blocks);
extern "C" void hirari_input_blocks_publish_monitor(void* blocks, const float* const* inputs,
                                                    uint32_t channel_count, uint32_t frames);
extern "C" void hirari_input_blocks_end_monitor(void* blocks);
extern "C" void hirari_input_blocks_publish_talkback(void* blocks, const float* const* inputs,
                                                     uint32_t channel_count, uint32_t frames,
                                                     const void* control_room);
extern "C" void hirari_input_blocks_end_talkback(void* blocks);

struct HirariScheduledMidiNote {
    uint32_t track_id;
    uint8_t pitch;
    uint8_t velocity;
    uint8_t midi_channel;
    uint8_t articulation_id;
    uint64_t start_sample;
    uint64_t length_samples;
    uint8_t probability;
    uint32_t region_id;
};
extern "C" uint64_t hirari_midi_snapshot_store_revision(const void* store);
using HirariScheduledMidiEventCallback = void (*)(
    void* context, const HirariScheduledMidiNote* note,
    uint64_t sample_offset, uint8_t note_off);
// Playback reconciliation state (discontinuity, snapshot revision, panic
// request) lives in Rust; the callback supplies the voices it must reset.
using HirariMidiPanicCallback = void (*)(void* context);
extern "C" void hirari_external_sync_set_rate(void* state, double bpm);
extern "C" double hirari_external_sync_rate(const void* state);
extern "C" void hirari_external_sync_tick(void* state, uint64_t timestamp);
extern "C" uint64_t hirari_external_sync_ticks(const void* state);
extern "C" uint64_t hirari_external_sync_last_tick(const void* state);
extern "C" void hirari_project_layout_fingerprint_update(
    uint64_t* state, const uint8_t* data, size_t length);
extern "C" uint64_t hirari_project_state_revision_mix(
    uint64_t layout, uint64_t midi, uint64_t control);
extern "C" void hirari_master_limiter_prepare(void* state, double sample_rate);
extern "C" void hirari_master_limiter_reset(void* state);
extern "C" void hirari_control_room_reset(void* state);
extern "C" void* hirari_bus_system_bus(const void* state, uint32_t bus_id);
extern "C" bool hirari_bus_system_register(void* state, uint32_t bus_id);
extern "C" void hirari_bus_system_reset(void* state);
extern "C" void* hirari_audio_buffer_storage_create();
extern "C" void hirari_audio_buffer_storage_destroy(void* storage);
extern "C" uint8_t hirari_audio_buffer_storage_reserve_buffer(
    void* storage, uint32_t channels, uint32_t samples);
extern "C" bool hirari_audio_buffer_storage_resize(
    void* storage, uint32_t channels, uint32_t samples);
extern "C" bool hirari_audio_buffer_storage_wrap_channels(
    void* storage, float** channels, uint32_t channel_count, uint32_t sample_count);
extern "C" uint32_t hirari_audio_buffer_storage_channels(const void* storage);
extern "C" uint32_t hirari_audio_buffer_storage_samples(const void* storage);
extern "C" float* hirari_audio_buffer_storage_write_pointer(
    const void* storage, uint32_t channel);
extern "C" const float* hirari_audio_buffer_storage_read_pointer(
    const void* storage, uint32_t channel, uint32_t offset);
extern "C" void hirari_audio_buffer_storage_clear(
    const void* storage, uint32_t offset, uint32_t samples);
extern "C" uint32_t hirari_audio_buffer_storage_sanitize(const void* storage);
extern "C" void hirari_audio_buffer_set_realtime_guard(bool is_realtime);
extern "C" uint64_t hirari_audio_buffer_realtime_resize_attempts();
// One block's MIDI event, mirrored field for field from the Rust owner's
// `#[repr(C)]` layout. 256 payload bytes cover MIDI 2.0 UMP packets and bounded
// SysEx chunks; anything larger is rejected and counted, never truncated.
struct HirariMidiEvent {
    uint64_t sampleOffset;
    uint32_t size;
    uint8_t data[256];
    uint8_t articulationId = 0;  // 0 = default, 1+ = technique id
};
static_assert(offsetof(HirariMidiEvent, sampleOffset) == 0);
static_assert(offsetof(HirariMidiEvent, size) == 8);
static_assert(offsetof(HirariMidiEvent, data) == 12);
static_assert(offsetof(HirariMidiEvent, articulationId) == 268);
static_assert(sizeof(HirariMidiEvent) == 272);
// `MidiBufferState::kMaxEvents` in the Rust owner.
constexpr size_t kHirariMidiMaxEventsPerBlock = 1024;

extern "C" void* hirari_midi_buffer_create();
extern "C" void hirari_midi_buffer_destroy(void* state);
extern "C" void hirari_midi_buffer_add(
    void* state, uint64_t sample_offset, const uint8_t* data, uint32_t size,
    uint8_t articulation_id);
// Channel validation and status-byte construction are Rust-owned.
extern "C" void hirari_midi_buffer_add_note_on(void* state, uint8_t channel, uint8_t pitch, uint8_t velocity, uint64_t sampleOffset, uint8_t articulationId);
extern "C" bool hirari_midi_buffer_copy(void* state, const void* event);
extern "C" void* hirari_midi_buffer_event_data(const void* state);
extern "C" size_t hirari_midi_buffer_event_count(const void* state);
extern "C" bool hirari_midi_buffer_overflowed(const void* state);
extern "C" bool hirari_midi_buffer_take_overflowed(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_dropped(const void* state);
extern "C" uint64_t hirari_midi_buffer_dropped_count(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_oversize(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_extended(const void* state);


struct HirariRegionRangeEdit {
    uint64_t start;
    uint64_t end;
    float gain;
    uint64_t fade_in;
    uint64_t fade_out;
};
struct HirariRegionState;
// VariAudio note-segment ownership. Rust now owns the note-edit table; C++
// keeps only a published immutable projection and the project-file layout.
typedef struct HirariRegionVariAudioStore HirariRegionVariAudioStore;

struct HirariAudioNoteAnchor;
struct HirariAudioNoteCurveView;
typedef struct HirariVariAudioFrozen HirariVariAudioFrozen;

// Authoritative note-edit store. Rust-owned; C++ consumes it through this
// narrow, allocation-free handle.
extern "C" {
struct HirariAudioNoteSegmentRange;

}

// Region range edits are a pure transform: Rust reads the caller's current
// edits and returns a buffer it owns. Nothing is written through a pointer into
// caller storage, so no returned count can drive a container resize over data
// a foreign language had already written.
struct HirariRegionCompRange {
    uint64_t start;
    uint64_t end;
    uint64_t fade_in;
    uint64_t fade_out;
};

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
typedef struct HirariRegionProcessingHistoryStore HirariRegionProcessingHistoryStore;
struct HirariWarpMarker {
    uint64_t sourceSample = 0;
    uint64_t timelineSample = 0;
    bool transient = false;
};
// Warp-marker normalization is a pure transform owned by the range-edit store;
// no C++ caller transforms buffers directly.
// Per-region range-edit, comp-range and warp-marker storage. Rust owns the
// tables; a frozen projection is what a published snapshot and the audio
// callback read.
struct HirariRegionClipEdits;
struct HirariRegionClipEditsFrozen;
struct HirariRegionClipEditsView {
    const HirariRegionRangeEdit* rangeEdits = nullptr;
    size_t rangeEditCount = 0;
    const HirariRegionCompRange* compRanges = nullptr;
    size_t compRangeCount = 0;
    const HirariWarpMarker* warpMarkers = nullptr;
    size_t warpMarkerCount = 0;
    uint8_t compManaged = 0;
};
// Region edit host bridge.
//
// The region table lives in Rust (region_store.rs); the engine keeps only the
// handles the store cannot own — decoded audio buffers and the publish target.
// Rust owns the edit policy: it reads the current state, decides, applies and
// records exactly one undo entry, all inside the store. Every callback runs on
// the control thread; none of them is reachable from the audio callback.
//
// The structs below mirror `hirari-core-bridge/src/region_edit_host.rs` byte
// for byte. Keep the field order and the primitive types in sync.
struct HirariRegionState {
    uint64_t start = 0;
    uint64_t len = 0;
    uint64_t baseStart = 0;
    uint64_t baseLength = 0;
    uint64_t sourceOffset = 0;
    uint64_t baseSourceOffset = 0;
    uint64_t sourceLength = 0;
    uint64_t fadeInSamples = 0;
    uint64_t fadeOutSamples = 0;
    double warpRatio = 1.0;
    double sourceSampleRate = 0.0;
    double timelineSampleRate = 0.0;
    float clipGain = 1.0f;
    float pitchSemitones = 0.0f;
    uint32_t loopCount = 1;
    uint32_t syncGroup = 0;
    bool muted = false;
    bool reverse = false;
    bool locked = false;
    bool pitchPreserveWarp = false;
};

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

enum HirariRegionEditOp : int32_t {
    // Replace the editable state and republish it (normalising add path).
    kHirariRegionEditState = 1,
    // Write the policy-controlled scalar fields in place.
    kHirariRegionEditInPlace = 2,
    // Restore a whole note table from a Rust-owned snapshot handle.
    kHirariRegionEditNoteStore = 11,
    // Remove the region and drop its cached peaks.
    kHirariRegionEditRemove = 12,
    // Put a removed region back from a host-held snapshot handle (blob).
    kHirariRegionEditRestore = 13,
    // Write the geometry fields Rust computed for a trim or warp-ratio edit
    // and replace the warp markers from the blob.
    kHirariRegionEditGeometry = 14,
    // Replace only the warp markers from the blob (timing jobs).
    kHirariRegionEditMarkers = 15,
    // Store the range-edit list packed in the blob exactly as given.
    kHirariRegionEditRangeEdits = 16,
    // Store the comp-range batch packed in the blob plus `managed` exactly as
    // given.
    kHirariRegionEditCompRanges = 17,
    // Swap a region's audio for a decoded handle. The blob is a native-layout
    // header (audio handle, source rate, timeline rate) followed by the path
    // bytes; the host takes the handle over on every path.
    kHirariRegionEditReplaceAudio = 18,
    // Commit an owned F0 analysis result (blob) into the region's note table
    // when the region still matches `state` at `generation`. The host takes
    // the analysis handle over on every path.
    kHirariRegionEditPitchCommit = 19,
};

struct HirariRegionEditPayload {
    uint32_t trackId = 0;
    uint32_t regionId = 0;
    int32_t op = kHirariRegionEditInPlace;
    // Packed payload or, for kHirariRegionEditNoteStore, an owned snapshot
    // handle the host takes over.
    const uint8_t* blob = nullptr;
    size_t blobLen = 0;
    HirariRegionState state;
    // Comp-range batch flag; unused by the other ops.
    bool managed = false;
    // Region-table generation the payload was decided against; 0 skips the
    // staleness check. Only the pitch commit reads it.
    uint64_t generation = 0;
};

struct HirariRegionGroupMember {
    uint32_t trackId = 0;
    uint32_t regionId = 0;
    uint64_t start = 0;
    uint64_t baseStart = 0;
    bool locked = false;
};

// Runs one note-table edit while the host holds the track's region lock, then
// publishes the snapshot. The table itself is Rust-owned, so the host never
// interprets the mutation.
using HirariRegionNoteMutateFn = bool (*)(void* context, HirariRegionVariAudioStore* store);

// The lifetime handle of an imported Rust audio buffer is Rust's own
// (storage_handle.rs): the region store views its planes and releases the
// handle through these entry points.
extern "C" uint32_t hirari_storage_handle_view(void* audio, const float** out,
                                               size_t capacity, uint64_t* samples);
extern "C" void hirari_storage_handle_release(void* audio);

struct HirariRegionHostOps {
    bool (*findRegion)(void* host, uint32_t trackId, uint32_t regionId,
                       HirariRegionState* out) = nullptr;
    bool (*applyEdit)(void* host, const HirariRegionEditPayload* payload) = nullptr;
    size_t (*listSyncGroup)(void* host, uint32_t group, HirariRegionGroupMember* out,
                            size_t capacity) = nullptr;
    bool (*recordUndo)(void* host, const char* name,
                       const HirariRegionEditPayload* undo, size_t undoCount,
                       const HirariRegionEditPayload* redo, size_t redoCount,
                       bool coalesce, bool perform) = nullptr;
    // Runs a Rust note-table mutation under the region lock and publishes.
    bool (*noteEdit)(void* host, uint32_t trackId, uint32_t regionId, void* context,
                     HirariRegionNoteMutateFn mutate) = nullptr;
    // Copies a whole region into an owned handle for undo; null if missing.
    void* (*snapshotRegion)(void* host, uint32_t trackId, uint32_t regionId) = nullptr;
    // Frees a snapshot handle that was never recorded for undo.
    void (*releaseSnapshot)(void* host, void* handle) = nullptr;
    // Copies a region's warp markers into `out`; returns the total count.
    size_t (*warpMarkers)(void* host, uint32_t trackId, uint32_t regionId,
                          HirariWarpMarker* out, size_t capacity) = nullptr;
    // Copies a region's range edits into `out`; returns the total count.
    size_t (*rangeEdits)(void* host, uint32_t trackId, uint32_t regionId,
                         HirariRegionRangeEdit* out, size_t capacity) = nullptr;
    // Retains a region's decoded audio so a worker can read it later.
    void* (*retainAudio)(void* host, uint32_t trackId, uint32_t regionId) = nullptr;
    // Reports a retained handle's channels, sample count and pointers.
    uint32_t (*audioView)(void* handle, const float** out, size_t capacity,
                          uint64_t* samples) = nullptr;
    void (*releaseAudio)(void* handle) = nullptr;
    // Reports a track's region generation and published snapshot identity.
    bool (*trackToken)(void* host, uint32_t trackId, uint64_t* generation,
                       size_t* snapshot) = nullptr;
    // Lists track ids / one track's region ids in storage order.
    size_t (*listTracks)(void* host, uint32_t* out, size_t capacity) = nullptr;
    size_t (*listRegions)(void* host, uint32_t trackId, uint32_t* out,
                          size_t capacity) = nullptr;
    void* (*tempoState)(void* host) = nullptr;
    void* (*undoState)(void* host) = nullptr;
    double (*sampleRate)(void* host) = nullptr;
    // Copies a region's source path into `out`; returns its byte length.
    size_t (*regionPath)(void* host, uint32_t trackId, uint32_t regionId,
                         uint8_t* out, size_t capacity) = nullptr;
    // Inserts a decoded region under the id the caller allocated. The audio
    // handle is consumed on every path.
    uint32_t (*insertRegion)(void* host, uint32_t trackId, size_t expectedTrack,
                             const uint8_t* path, size_t pathLength, uint64_t startSample,
                             double sourceSampleRate, double timelineSampleRate,
                             uint64_t length, uint32_t regionId, void* audioHandle) = nullptr;
    // Live track identity, or 0 when the id is not in the project.
    size_t (*trackIdentity)(void* host, uint32_t trackId) = nullptr;
    // Retains a track for a worker thread.
    void* (*retainTrack)(void* host, uint32_t trackId) = nullptr;
    void (*releaseTrack)(void* handle) = nullptr;
    bool (*pitchBegin)(void* trackHandle) = nullptr;
    void (*pitchFinish)(void* trackHandle) = nullptr;
    // Copies a region's note table into an owned Rust store handle.
    void* (*noteSnapshot)(void* trackHandle, uint32_t regionId) = nullptr;
    // Clones a region under the id the caller allocated, optionally replacing
    // its editable state first, and adds the copy. Returns 0 on failure.
    uint32_t (*cloneRegion)(void* host, uint32_t trackId, uint32_t regionId,
                            const HirariRegionState* state, uint32_t newRegionId) = nullptr;
    // Copies a region's comp ranges into `out` and reports the managed flag.
    // Returns the total count, with the same count-then-copy contract as the
    // range-edit listing.
    size_t (*compRanges)(void* host, uint32_t trackId, uint32_t regionId,
                         HirariRegionCompRange* out, size_t capacity,
                         bool* managed) = nullptr;
};

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
extern "C" bool hirari_audio_device_mode_offline();
extern "C" bool hirari_native_test_isolation();
// Audio configuration bounds (audio_config_rules.rs).
extern "C" bool hirari_audio_format_valid(double sampleRate, uint32_t blockSize);
extern "C" void* hirari_region_waveform(
    void* host, uint32_t trackId, uint32_t regionId, uint32_t numPeaks);
extern "C" void hirari_region_waveform_clear(void* host);
// Decodes `path`, swaps it into the region and records the undo pair. Rust
// owns the path/rate policy and the snapshots; C++ performs the swap.

// Automation edit host. Rust reads and writes the curves in each track's own
// store, so the engine contributes only the undo history.
struct HirariAutomationHostOps {
    void* (*undoState)(void* host) = nullptr;
};

extern "C" void hirari_automation_host_install(const HirariAutomationHostOps* ops);

struct HirariRegionBlockConfig {
    uint64_t regionOffset = 0;
    uint64_t regionLength = 0;
    uint64_t timelineLength = 0;
    uint64_t fadeIn = 0;
    uint64_t fadeOut = 0;
    uint64_t crossfadeIn = 0;
    uint64_t crossfadeOut = 0;
    float clipGain = 1.0f;
    uint8_t compManaged = 0;
    double sampleRate = 0.0;
    double sourceRate = 0.0;
    uint64_t sourceSpan = 0;
};
struct HirariRegionBlockOutput {
    float* gain = nullptr;
    double* pitchCents = nullptr;
    double* formantCents = nullptr;
    int64_t* matchedNoteIndices = nullptr;
    int64_t* previousNoteIndices = nullptr;
    double* sourcePositions = nullptr;
    double* localSourceRates = nullptr;
};
struct HirariRegionRenderConfig {
    uint64_t sourceSamples = 0;
    uint64_t sourceOffset = 0;
    uint64_t sourceSpan = 0;
    double sourceRate = 1.0;
    double sampleRate = 44100.0;
    double basePitchRatio = 1.0;
    double minimumDelay = 256.0;
    double delayRange = 256.0;
    uintptr_t cacheOwner = 0;
    uintptr_t cacheSnapshot = 0;
    uintptr_t cacheSource = 0;
    uint32_t cacheRegionId = 0;
    uintptr_t cacheNoteSegments = 0;
    size_t cacheNoteSegmentCount = 0;
    float cachePitchSemitones = 0.0f;
    uint32_t syncGroup = 0;
    uint8_t reverse = 0;
    uint8_t pitchPreserveWarp = 0;
    uint8_t spectralStretchReady = 0;
};
struct HirariAudioNoteAnchor {
    double positionSeconds = 0.0;
    double pitchCents = 0.0;
    double formantCents = 0.0;
};
struct HirariAudioNoteCurveView {
    const HirariAudioNoteAnchor* anchors = nullptr;
    size_t anchorCount = 0;
    const double* integralPrefix = nullptr;
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double pitchOffsetCents = 0.0;
    double correctionBeforeSeconds = 0.0;
    double formantOffsetCents = 0.0;
};
struct HirariAudioNoteSegmentRange {
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double detectedPitchCents = 0.0;
};
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
extern "C" bool hirari_tempo_map_add_tempo(
    void* state, uint64_t sample_pos, double bpm, double sample_rate, bool ramp);
extern "C" bool hirari_tempo_map_clear(void* state, double sample_rate, double initial_bpm);
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
extern "C" bool hirari_routing_engine_add_connection(
    void* state, uint32_t source, uint32_t destination, float gain, bool send, bool pre_fader);

// Routing edit host bridge.
//
// Rust owns validation, cycle/feedback rejection, send-vs-insert resolution
// and the undo capture; the engine owns the live routing objects and applies
// the resolved mutations under its control locks. Keep this layout in sync
// with hirari-core-bridge/src/routing_edit_host.rs.

struct HirariRoutingEditPayload {
    int32_t op = 0;
    uint32_t sourceId = 0;
    uint32_t destId = 0;
    uint32_t pluginIndex = 0;
    uint32_t tapPoint = 0;
    float gain = 0.0f;
    uint32_t samples = 0;
    bool enabled = false;
    bool preFader = false;
};

struct HirariRoutingSidechainContext {
    uint32_t blockSize = 0;
    uint32_t sampleRate = 0;
    uint64_t generation = 0;
    bool sourceFound = false;
    bool destinationFound = false;
    bool sourcePrepared = false;
};

struct HirariRoutingHostOps {
    // The routing graph, the sidechain route table and the sidechain gate are
    // Rust objects the engine only holds, so Rust reads and edits them through
    // its own store API. These accessors are the remaining object-graph reach.
    void* (*routingStore)(void*) = nullptr;
    void* (*sidechainRoutesStore)(void*) = nullptr;
    void* (*sidechainEngineStore)(void*) = nullptr;
    // The engine installs neither of these; Rust derives the registration
    // inputs from the engine block state and taps the source's work buffer
    // through the track session. A host without that state may supply them.
    bool (*sidechainContext)(void*, uint32_t, uint32_t,
                             HirariRoutingSidechainContext*) = nullptr;
    bool (*registerSidechain)(void*, uint32_t, uint32_t, uint32_t,
                              const HirariRoutingSidechainContext*, uint32_t, bool) = nullptr;
    void (*markLayoutChanged)(void*) = nullptr;
    // Rebuilds the published track/PDC graph; takes the tracks lock itself.
    void (*syncStructural)(void*) = nullptr;
    void (*recordUndo)(void*, const char*, const HirariRoutingEditPayload*, size_t,
                       const HirariRoutingEditPayload*, size_t, bool, bool) = nullptr;
    void* (*undoState)(void*) = nullptr;
    // The engine block state records the device format the graph was last
    // prepared for; the sidechain registration inputs read it from there.
    void* (*engineBlockStore)(void*) = nullptr;
};

extern "C" void hirari_routing_edit_host_install(const HirariRoutingHostOps* ops);
extern "C" bool hirari_routing_edit_set_sidechain_link(
    void* host, uint32_t sourceId, uint32_t destId, uint32_t pluginIndex, uint32_t tapPoint,
    bool enabled);

// Control-thread edit policy (input monitoring, macro learn). The names live in
// each track's own Rust store, so the engine supplies no naming adapter.
struct HirariControlEditHostOps {
    bool (*updateMonitorFocus)(void*, uint32_t, bool, uint32_t, uint32_t) = nullptr;
    void* (*macroControlStore)(void*) = nullptr;
    void* (*midiLearnStore)(void*) = nullptr;
    void* (*undoState)(void*) = nullptr;
};

extern "C" void hirari_control_edit_host_install(const HirariControlEditHostOps* ops);

// One control-thread session over a live track (track_session.rs). Rust owns
// the edit protocol — resolve the track, mutate its own Rust stores, drop the
// frozen render, bump the layout revision — so an edit host needs no callbacks
// of its own. These are the only steps that are genuinely engine code: the lock
// the tracks live behind, the handles a Track physically owns, re-preparing a
// chain for playback and marking the project layout dirty.
struct HirariTrackSessionOps {
    void (*lockTracks)(void*) = nullptr;
    void (*unlockTracks)(void*) = nullptr;
    // Resolves a live track under the held lock; null when it is gone.
    void* (*liveTrack)(void*, uint32_t) = nullptr;
    /// One of a track's own storages: 0 channel strip, 1 effect chain,
    /// 2 automation store, 3 freeze store, 4 realtime runtime,
    /// 5 MIDI articulation state — the numbering the layout gather uses.
    void* (*trackStorage)(void*, uint32_t) = nullptr;
    uint32_t (*trackType)(void*) = nullptr;
    void (*markTrackLayout)(void*) = nullptr;
    // Re-prepares the track at the engine's current rate and block size.
    void (*prepareTrack)(void*, void*) = nullptr;
};

extern "C" void hirari_track_session_install(const HirariTrackSessionOps* ops);

// Plugin-chain edit policy (bypass, reordering, parameters and presets). The
// chain, the frozen render and the layout revision are read and written by Rust
// through the track session above; only these two steps are engine code.
struct HirariPluginEditHostOps {
    // Re-publishes the track snapshot after the chain changed.
    void (*syncStructuralChanges)(void*) = nullptr;
    // Returns the engine's Rust undo history handle.
    void* (*undoState)(void*) = nullptr;
};

extern "C" void hirari_plugin_edit_host_install(const HirariPluginEditHostOps* ops);

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

// Track and region id allocation lives in Rust (id_host.rs). An emptied
// project restarts the counters; an installed document resumes them past the
// highest ids it carries. Both are store-only and safe under the track lock.
extern "C" void hirari_ids_reset_project(void* host);
extern "C" void hirari_ids_seed_from_project(void* host);

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

// Control-command admission (target and generations). The command ring and the
// forensic journal are Rust objects the engine only holds, so they travel as
// handles and the queue-full diagnostic is posted to the Rust log directly.
struct HirariCommandHostOps {
    bool (*trackExists)(void*, uint32_t) = nullptr;
    // Writes the current project and audio-configuration generations.
    bool (*generations)(void*, uint64_t*, uint64_t*) = nullptr;
    void* (*commandRings)(void*) = nullptr;
    void* (*journaler)(void*) = nullptr;
};
extern "C" void hirari_command_host_install(const HirariCommandHostOps* ops);
extern "C" bool hirari_engine_push_command(void* host, uint32_t type, uint32_t trackId,
                                           float value, uint64_t timestamp,
                                           uint64_t expectedProjectGeneration,
                                           uint64_t expectedAudioGeneration);

// Track control policy (spatial validation, HRTF admission, auto-mix curve)
// is Rust's; it pins a track through the track session host below.
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
extern "C" uint64_t hirari_layout_revision_current(void* state);
// Builds one track and returns its owning handle; `out_track` receives the
// object pointer. The caller gives the handle to the track table.
extern "C" void* hirari_track_new(uint32_t id, const uint8_t* name, size_t name_len,
                                  uint32_t kind, void* revision, void** out_track);
extern "C" void hirari_track_owner_release(void* owner);
extern "C" void* hirari_track_owner_object(void* owner);
extern "C" uint32_t hirari_track_id(void* track);
extern "C" uint32_t hirari_track_type(void* track);
extern "C" void hirari_track_resolve_bus(void* track, void* bus_state, bool is_bus);
extern "C" void* hirari_track_storage(void* track, uint32_t store);
extern "C" void hirari_track_mark_layout(void* track);
extern "C" uint8_t hirari_track_prepare(void* track, double sample_rate,
                                        uint32_t block_frames);
extern "C" bool hirari_track_can_process(void* track, uint32_t frames);
extern "C" void hirari_track_reset_for_offline_render(void* track);
extern "C" void hirari_track_set_delay_samples(void* track, uint32_t samples);
extern "C" void hirari_track_set_volume(void* track, float value);
extern "C" void hirari_track_set_pan(void* track, float value);
extern "C" void hirari_track_set_solo(void* track, bool solo);
extern "C" void hirari_track_set_muted(void* track, bool muted);
extern "C" void hirari_track_set_phase_inverted(void* track, bool inverted);
extern "C" bool hirari_track_set_record_armed(void* track, bool armed);
extern "C" void hirari_track_set_spatial(void* track, float x, float y, float z);
extern "C" uint32_t hirari_track_flush_recorded_automation(void* track);
extern "C" bool hirari_track_pitch_begin(void* track);
extern "C" void hirari_track_pitch_finish(void* track);
extern "C" bool hirari_track_expression_map_set(void* track, uint8_t kind,
                                                const uint8_t* data, size_t length);
extern "C" void hirari_track_reclaim_chain(void* track);
extern "C" bool hirari_track_visit_sandbox_statuses(void* track, void* context,
                                                    HirariPluginSandboxVisitorFn visit);
extern "C" uint32_t hirari_track_maintain_sandboxes(void* track, bool autoRestart,
                                                    void* context,
                                                    HirariPluginSandboxVisitorFn visit);

extern "C" void hirari_native_pdc_reset_for_project(const void* state);

extern "C" bool hirari_vocal_remove_stereo(float* left, float* right, size_t frames);
extern "C" float hirari_stereo_correlation(const float* left, const float* right, size_t frames);
// Arrangement timeline document (markers and arranger parts) is a Rust-owned
// store; the engine only keeps the handle and the save path lets Rust read
// the document directly.
struct HirariArrangementPartInput {
    uint64_t start;
    uint64_t length;
    const uint8_t* name;
    size_t name_len;
    uint32_t repeats;
};
extern "C" void hirari_arrangement_document_reset_markers(void* state);
extern "C" bool hirari_arrangement_document_upsert_marker(
    void* state, uint64_t sample, const uint8_t* name, size_t name_len, uint32_t color);
extern "C" bool hirari_arrangement_document_set_parts(
    void* state, const HirariArrangementPartInput* parts, size_t count);
struct HirariTimedKeySwitchOutput {
    uint8_t channel;
    uint8_t pitch;
    uint32_t length_ticks;
};
extern "C" uint32_t hirari_automation_record_mode_apply(uint32_t mode);
struct HirariTimedParameterEvent {
    uint32_t processor_index;
    uint32_t parameter_id;
    uint32_t sample_offset;
    float normalized_value;
};
static_assert(sizeof(HirariTimedParameterEvent) == 16);
/// Runs the published node list over one block. The channel table and frame
/// count come from the caller's own audio-buffer storage handle.
struct HirariEffectChainSandboxStatus {
    uint32_t plugin_index;
    uint8_t alive;
    uint8_t can_retry;
    uint8_t failure;
    uint8_t recovery_mode;
    uint32_t dropped_output_midi;
    uint32_t mailbox_overruns;
    uint32_t input_midi_truncations;
};
static_assert(sizeof(HirariEffectChainSandboxStatus) == 20,
    "sandbox status must match Rust's repr(C) layout");


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
extern "C" void* hirari_engine_session_create();
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
extern "C" void hirari_engine_block_set_auto_record_mode(void* state, uint32_t mode);
extern "C" uint8_t hirari_engine_block_is_shutdown(const void* state);
/// Sets the shutdown flag and reports whether it was already set.
extern "C" uint8_t hirari_engine_block_take_shutdown(void* state);
extern "C" void hirari_engine_block_callback_enter(void* state);
extern "C" void hirari_engine_block_callback_leave(void* state);
/// Waits for every callback in flight; a control path, never the realtime one.
extern "C" void hirari_engine_block_join_callbacks(const void* state);
extern "C" uint64_t hirari_engine_block_control_revision(const void* state);
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

extern "C" bool hirari_status_queue_push_default(
    uint32_t severity, const uint8_t* text, size_t text_size);
extern "C" bool hirari_status_queue_pop_default(void* output_message);
extern "C" uint64_t hirari_status_queue_take_dropped_default();
extern "C" void* hirari_undo_manager_create(size_t max_depth, uint64_t coalesce_window_ms);
extern "C" uint64_t hirari_undo_manager_register_native_callback(
    void* state, void* context, void (*invoke)(void*), void (*destroy)(void*));
extern "C" bool hirari_undo_manager_abort_and_invoke(void* state);
extern "C" void hirari_undo_manager_destroy(void* state);
extern "C" bool hirari_undo_manager_commit(
    void* state, const uint8_t* name, size_t name_len,
    uint64_t undo_callback, uint64_t redo_callback, bool coalesce, bool perform);
extern "C" bool hirari_undo_manager_apply(void* state, bool redo);
extern "C" bool hirari_undo_manager_begin_transaction(
    void* state, const uint8_t* name, size_t name_len);
extern "C" bool hirari_undo_manager_end_transaction(void* state);
extern "C" bool hirari_undo_manager_transaction_active(const void* state);
extern "C" size_t hirari_undo_manager_undo_count(const void* state);
extern "C" size_t hirari_undo_manager_redo_count(const void* state);
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
extern "C" void* hirari_project_db_create();
extern "C" void hirari_project_db_destroy(void* state);
extern "C" bool hirari_project_db_record(
    void* state, uint64_t timestamp,
    const uint8_t* category, size_t category_size,
    const uint8_t* key, size_t key_size,
    const uint8_t* value, size_t value_size);
extern "C" bool hirari_project_db_flush(
    const void* state, const uint8_t* path, size_t path_size);
extern "C" void hirari_mpe_manager_reset(void* state);


using HirariSnapshotDestroyCallback = void (*)(void*);
using HirariWaveformComputeCallback = size_t (*)(void*, uint32_t, uint32_t, float*, size_t);
// Retires the Rust layout-snapshot session bound to this engine and joins its
// worker, so a background save can never outlive the graph it reads.
extern "C" void hirari_layout_snapshot_owner_release_host(void* host);
extern "C" uint64_t hirari_waveform_snapshot_request(
    void* context, uint32_t track_id, uint32_t region_id,
    HirariWaveformComputeCallback compute, HirariSnapshotDestroyCallback destroy_context);

// Project-layout gathering. Rust walks the live engine model through this
// table (layout_gather.rs), owns the snapshot and its backing blocks, and
// serializes or saves it in the same call. `begin` takes the tracks lock and
// drains recorded automation; `end` releases it on every path. The gather then
// addresses each live track and reads its model through the Rust stores the
// track owns; only the track name is C++ state.
using HirariLayoutIsCancelledCallback = bool (*)(const void*);
struct HirariLayoutHostOps {
    void (*begin)(void* host) = nullptr;
    void (*end)(void* host) = nullptr;
    size_t (*trackCount)(void* host) = nullptr;
    void* (*trackHandle)(void* host, size_t index) = nullptr;
    // Every per-track field the layout needs is a Rust storage already.
    uint32_t (*trackId)(void* track) = nullptr;
    uint32_t (*trackType)(void* track) = nullptr;
    /// One of a track's own storages: 0 channel strip, 1 effect chain,
    /// 2 automation store, 3 freeze store, 4 realtime runtime,
    /// 5 MIDI articulation state, 6 name store. Rust reads them with its own
    /// store API.
    void* (*trackStorage)(void* track, uint32_t kind) = nullptr;
    void* (*sidechainRoutesStore)(void* host) = nullptr;
    void* (*routingStore)(void* host) = nullptr;
    double (*sampleRate)(void* host) = nullptr;
};
// Both calls gather, serialize and release inside Rust. The JSON buffer is
// Rust-owned and must be released with hirari_project_layout_json_free.
extern "C" bool hirari_layout_json_from_host(
    void* host, const HirariLayoutHostOps* ops,
    HirariLayoutIsCancelledCallback is_cancelled, const void* cancel_context,
    uint8_t** json_data, size_t* json_size);
extern "C" void hirari_project_layout_json_free(const uint8_t* json_data, size_t json_size);
extern "C" bool hirari_project_save_from_host(
    void* host, const HirariLayoutHostOps* ops,
    const uint8_t* path, size_t path_length,
    const void* tempo_state, const void* routing_state, size_t arrangement_document,
    double bpm, const void* engine_block);

// Rust-owned harmonic context shared by structure analysis and auto-arrangement.
extern "C" void hirari_harmonic_context_get(
    float* tension, float* valence, float* stability);
