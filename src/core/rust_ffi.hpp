#pragma once

// `size_t` is used by the limits below, so the header that defines it has to
// come first. This include used to sit *after* the namespace, which compiled
// only as long as every includer happened to pull in <cstddef> transitively;
// the spectral-editor and mmap-audio contracts reached rust_ffi.hpp first and
// failed on `unknown type name 'size_t'`.
#include <cstddef>
#include <cstdint>
#include <type_traits>

#include "project_automation.hpp"

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
extern "C" uint64_t hirari_log_buffer_timestamp();
extern "C" void hirari_log_buffer_post(
    const void* state, uint32_t level, uint32_t component_id,
    const uint8_t* message, size_t message_len);
extern "C" bool hirari_log_buffer_pop(const void* state, void* output);

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
struct HirariMidiSnapshotView {
    const HirariScheduledMidiNote* notes;
    size_t note_count;
    const size_t* indices_by_end;
    size_t end_count;
    size_t slot;
    uint64_t revision;
};
extern "C" void* hirari_midi_snapshot_store_create();
extern "C" void hirari_midi_snapshot_store_destroy(void* store);
extern "C" bool hirari_midi_snapshot_store_pin(
    const void* store, HirariMidiSnapshotView* output);
extern "C" void hirari_midi_snapshot_store_release(const void* store, size_t slot);
extern "C" uint64_t hirari_midi_snapshot_store_revision(const void* store);
extern "C" uint64_t hirari_midi_scheduled_note_end(const HirariScheduledMidiNote* note);
extern "C" HirariScheduledMidiNote* hirari_midi_notes_snapshot_take(
    const void* store, size_t* count);
extern "C" void hirari_midi_notes_snapshot_take_free(HirariScheduledMidiNote*, size_t);
extern "C" size_t hirari_midi_notes_unpack(
    const uint64_t* packed, size_t len, HirariScheduledMidiNote* output, size_t capacity);
extern "C" size_t hirari_midi_notes_pack(
    const HirariScheduledMidiNote* notes, size_t note_count, uint64_t* output, size_t capacity);
extern "C" bool hirari_midi_notes_replace(
    void* store, const HirariScheduledMidiNote* notes, size_t count,
    bool record_undo, void* undo, const void* log);
extern "C" bool hirari_midi_notes_edit_range(
    void* store, uint32_t track_id, uint64_t start_sample, uint64_t end_sample,
    uint32_t operation, int64_t amount, void* undo, const void* log);
extern "C" void hirari_midi_notes_upsert(
    void* store, uint32_t track_id, uint8_t pitch, uint8_t velocity,
    uint64_t start_sample, uint64_t length_samples, void* undo, const void* log);
using HirariScheduledMidiEventCallback = void (*)(
    void* context, const HirariScheduledMidiNote* note,
    uint64_t sample_offset, uint8_t note_off);
extern "C" void hirari_midi_schedule_block(
    const HirariScheduledMidiNote* notes, size_t note_count,
    const size_t* indices_by_end, size_t end_count, uint64_t playhead,
    uint32_t frames, uint8_t playback_active, uint8_t reconcile,
    uint64_t pass, void* context, HirariScheduledMidiEventCallback callback);
extern "C" bool hirari_preview_sample_render(
    const float* samples, size_t sample_count, double source_rate,
    double target_rate, float* left, float* right, uint32_t frames,
    double* position);
extern "C" void hirari_mixer_track_peaks(
    const void* telemetry_state, const uint32_t* track_ids,
    const float* const* left_channels, const float* const* right_channels,
    uint32_t track_count, uint32_t frames, float* peaks_left, float* peaks_right);
extern "C" bool hirari_audio_test_tone_render(
    float* left, float* right, uint32_t frames, uint64_t playhead, double sample_rate);
extern "C" void* hirari_external_sync_create();
extern "C" void hirari_external_sync_destroy(void* state);
extern "C" void hirari_external_sync_set_rate(void* state, double bpm);
extern "C" double hirari_external_sync_rate(const void* state);
extern "C" void hirari_external_sync_tick(void* state, uint64_t timestamp);
extern "C" uint64_t hirari_external_sync_ticks(const void* state);
extern "C" uint64_t hirari_external_sync_last_tick(const void* state);
extern "C" void hirari_audio_buffer_clear(
    float* const* channels, uint32_t channel_count, uint32_t offset, uint32_t sample_count);
extern "C" uint32_t hirari_audio_buffer_sanitize_non_finite(
    float* const* channels, uint32_t channel_count, uint32_t sample_count);
extern "C" void hirari_audio_buffer_add_channels(
    float* const* destinations, const float* const* sources,
    uint32_t channel_count, uint32_t sample_count);
extern "C" void hirari_audio_add_sanitized_stereo(
    float* destination_left, float* destination_right,
    const float* source_left, const float* source_right, uint32_t frames);
extern "C" void hirari_project_layout_fingerprint_update(
    uint64_t* state, const uint8_t* data, size_t length);
extern "C" uint64_t hirari_project_state_revision_mix(
    uint64_t layout, uint64_t midi, uint64_t control);
extern "C" float hirari_samples_to_milliseconds(uint64_t samples, double sample_rate);
extern "C" void hirari_audio_buffer_apply_gain(
    float* const* channels, uint32_t channel_count, uint32_t sample_count, float gain);
extern "C" void* hirari_preview_synth_create();
extern "C" void hirari_preview_synth_destroy(void* state);
extern "C" void hirari_preview_synth_set_engine(void* state, uint32_t engine);
extern "C" void hirari_preview_synth_reset(void* state);
extern "C" void hirari_preview_synth_process(
    void* state, const void* events, size_t event_count, float* left, float* right,
    uint32_t frames, uint64_t playhead, double sample_rate, const void* macros);
extern "C" void* hirari_master_limiter_create(double sample_rate);
extern "C" void hirari_master_limiter_destroy(void* state);
extern "C" void hirari_master_limiter_prepare(void* state, double sample_rate);
extern "C" void hirari_master_limiter_reset(void* state);
extern "C" void hirari_master_limiter_process(
    void* state, float* left, float* right, size_t frames, bool has_right);
extern "C" void hirari_master_limiter_set_control(const void* state, uint32_t control, float value);
extern "C" void hirari_master_limiter_set_parameter(const void* state, uint32_t id, float value);
extern "C" float hirari_master_limiter_parameter(const void* state, uint32_t id);
extern "C" size_t hirari_master_limiter_save_state(
    const void* state, uint8_t bypassed, float mix, uint32_t sidechain_bus_id,
    uint8_t* output, size_t capacity);
extern "C" bool hirari_master_limiter_restore_state(
    const void* state, const uint8_t* input, size_t length, uint8_t* bypassed,
    float* mix, uint32_t* sidechain_bus_id);
extern "C" uint32_t hirari_master_limiter_latency(const void* state);
extern "C" uint32_t hirari_master_limiter_tail(const void* state);
extern "C" void hirari_master_output_process(
    float* left, float* right, float* click_left, float* click_right,
    uint32_t frames, float master_gain, bool include_click);
extern "C" void hirari_control_room_mix_cue(
    const float* source_left, const float* source_right,
    const float* click_left, const float* click_right,
    float* output_left, float* output_right, uint32_t frames,
    float cue_gain, bool click_enabled);
extern "C" void* hirari_control_room_state_create();
extern "C" void hirari_control_room_state_destroy(void* state);
extern "C" void hirari_control_room_reset(void* state);
extern "C" void hirari_control_room_process_monitor_state(
    const void* state, float* left, float* right, const float* talkback, uint32_t frames);
extern "C" uint32_t hirari_control_room_active_cue_bus(const void* state);
extern "C" float hirari_control_room_active_cue_gain(const void* state);
extern "C" uint32_t hirari_control_room_active_cue_output(const void* state);
extern "C" bool hirari_control_room_active_cue_click(const void* state);
extern "C" bool hirari_control_room_is_cue_bus(const void* state, uint32_t id);
extern "C" bool hirari_bus_audio_accumulate(
    void* state, const float* input_left, const float* input_right,
    uint32_t frames, float gain);
extern "C" bool hirari_bus_audio_replace_post(
    void* state, const float* left, const float* right, uint32_t frames);
extern "C" bool hirari_bus_audio_read(
    void* state, float* left, float* right, uint32_t frames, bool post);
extern "C" void* hirari_bus_track_create();
extern "C" void hirari_bus_track_destroy(void* state);
extern "C" void hirari_bus_track_set_input_gain(void* state, float gain);
extern "C" void hirari_bus_track_set_phase_inverted(void* state, bool inverted);
extern "C" void hirari_bus_track_set_read_post_fx(void* state, bool post_fx);
extern "C" bool hirari_bus_track_fetch_audio(
    const void* track_state, void* bus_audio_state,
    float* left, float* right, uint32_t frames);
extern "C" void* hirari_plugin_parameter_snapshot_create();
extern "C" void hirari_plugin_parameter_snapshot_destroy(void* handle);
extern "C" bool hirari_plugin_parameter_snapshot_set(const void* handle, uint32_t parameter_id, double value);
extern "C" bool hirari_plugin_parameter_snapshot_get(const void* handle, uint32_t parameter_id, double* output);
using HirariPluginParameterVisitCallback = bool (*)(void* context, uint32_t parameter_id, double value);
extern "C" bool hirari_plugin_parameter_snapshot_visit(
    const void* handle, void* context, HirariPluginParameterVisitCallback callback);
extern "C" bool hirari_hot_swap_default_quiesce_requested();
extern "C" void hirari_hot_swap_default_acknowledge();
extern "C" void* hirari_bus_system_create();
extern "C" void hirari_bus_system_destroy(void* state);
extern "C" void* hirari_bus_system_bus(const void* state, uint32_t bus_id);
extern "C" bool hirari_bus_system_register(void* state, uint32_t bus_id);
extern "C" bool hirari_bus_system_reconcile(
    void* state, const bool* active, uint32_t count);
extern "C" void hirari_bus_system_reset(void* state);
extern "C" void hirari_bus_system_clear(const void* state, uint32_t frames);
extern "C" bool hirari_audio_buffer_copy(
    float* destination_left, float* destination_right,
    const float* source_left, const float* source_right, uint32_t sample_count);
extern "C" float hirari_audio_buffer_magnitude(const float* samples, uint32_t sample_count);
extern "C" void hirari_audio_buffer_storage_destroy(void* storage);
extern "C" float* hirari_audio_buffer_storage_data(const void* storage);
extern "C" size_t hirari_audio_buffer_storage_capacity(const void* storage);
extern "C" bool hirari_audio_buffer_storage_update_channel_pointers(
    void* storage, float* data, uint32_t channels, uint32_t samples);
extern "C" float** hirari_audio_buffer_storage_channel_pointers(const void* storage);
extern "C" uint8_t hirari_audio_buffer_storage_reserve_buffer(
    void** storage, uint32_t channels, uint32_t current_channels, uint32_t samples,
    size_t current_capacity,
    bool is_external, bool* needs_data_allocation);
extern "C" bool hirari_audio_buffer_storage_wrap_channels(
    void** storage, uint32_t* current_channels, uint32_t* current_samples,
    size_t* current_capacity, float** current_data, bool* is_external, float*** external_data,
    float** channels, uint32_t channel_count, uint32_t sample_count);
extern "C" void hirari_audio_buffer_set_realtime_guard(bool is_realtime);
extern "C" uint64_t hirari_audio_buffer_realtime_resize_attempts();
extern "C" size_t hirari_audio_buffer_last_realtime_requested_capacity();
extern "C" size_t hirari_audio_buffer_last_realtime_capacity();
extern "C" void hirari_audio_buffer_reset_realtime_resize_stats();
extern "C" bool hirari_audio_buffer_deinterleave_interleaved(
    const float* source, size_t source_sample_count,
    float* const* channel_pointers, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_macro_mapping_create();
extern "C" void hirari_macro_mapping_destroy(void* state);
extern "C" void hirari_macro_control_set_value(
    void* state, uint32_t macro_index, float value);
extern "C" void* hirari_channel_strip_create();
extern "C" void hirari_channel_strip_destroy(void* state);
extern "C" void hirari_channel_strip_set_sample_rate(const void* state, double sample_rate);
extern "C" double hirari_channel_strip_sample_rate(const void* state);
extern "C" void hirari_channel_strip_reset(const void* state);
extern "C" void hirari_channel_strip_set_gain(const void* state, float gain);
extern "C" void hirari_channel_strip_set_pan(const void* state, float pan);
extern "C" void hirari_channel_strip_set_mute(const void* state, bool muted);
extern "C" void hirari_channel_strip_set_solo(const void* state, bool solo);
extern "C" void hirari_channel_strip_process(
    const void* state, float* const* channels, uint32_t channel_count,
    uint32_t offset, size_t frames);
extern "C" void hirari_channel_strip_process_mirror(
    const void* state, float* const* channels, uint32_t channel_count,
    float* const* mirror_channels, uint32_t mirror_channel_count,
    uint32_t offset, size_t frames);
extern "C" void* hirari_midi_buffer_create();
extern "C" void hirari_midi_buffer_destroy(void* state);
extern "C" void hirari_midi_buffer_add(
    void* state, uint64_t sample_offset, const uint8_t* data, uint32_t size,
    uint8_t articulation_id);
// Channel validation and status-byte construction are Rust-owned.
extern "C" void hirari_midi_buffer_add_note_on(void* state, uint8_t channel, uint8_t pitch, uint8_t velocity, uint64_t sampleOffset, uint8_t articulationId);
extern "C" void hirari_midi_buffer_add_note_off(void* state, uint8_t channel, uint8_t pitch, uint64_t sampleOffset);
extern "C" void hirari_midi_buffer_add_all_notes_off(void* state, uint8_t channel, uint64_t sampleOffset);
extern "C" void* hirari_mixer_telemetry_create();
extern "C" void hirari_mixer_telemetry_destroy(void* state);
extern "C" bool hirari_midi_buffer_copy(void* state, const void* event);
extern "C" void hirari_midi_buffer_clear(void* state);
extern "C" void hirari_midi_buffer_sort_owned(void* state);
extern "C" void* hirari_midi_buffer_event_data(const void* state);
extern "C" size_t hirari_midi_buffer_event_count(const void* state);
extern "C" size_t hirari_midi_buffer_remaining_capacity(const void* state);
extern "C" bool hirari_midi_buffer_overflowed(const void* state);
extern "C" bool hirari_midi_buffer_take_overflowed(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_dropped(const void* state);
extern "C" uint64_t hirari_midi_buffer_dropped_count(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_oversize(const void* state);
extern "C" uint64_t hirari_midi_buffer_take_extended(const void* state);
extern "C" void hirari_midi_buffer_reject_extended(const void* state);
extern "C" bool hirari_processor_state_encode(
    bool bypassed, float mix, uint32_t sidechain_bus,
    uint8_t* output, size_t output_len);
extern "C" bool hirari_processor_state_decode(
    const uint8_t* input, size_t input_len, uint8_t* bypassed_out,
    float* mix_out, uint32_t* sidechain_bus_out);
struct HirariRegionRangeEdit {
    uint64_t start;
    uint64_t end;
    float gain;
    uint64_t fade_in;
    uint64_t fade_out;
};
struct HirariNativeRegionGeometry {
    uint64_t start;
    uint64_t length;
    uint64_t source_length;
    uint64_t source_offset;
    uint64_t base_start;
    uint64_t base_source_offset;
    uint64_t base_length;
    uint32_t loop_count;
    uint64_t fade_in;
    uint64_t fade_out;
    double warp_ratio;
    double source_sample_rate;
    double timeline_sample_rate;
};
struct HirariNativeRegionAddMetadata {
    uint64_t start;
    uint64_t length;
    uint64_t source_length;
    uint64_t source_offset;
    uint64_t base_start;
    uint64_t base_source_offset;
    uint64_t base_length;
    uint32_t loop_count;
    float clip_gain;
    double warp_ratio;
    float pitch_semitones;
};
extern "C" bool hirari_region_normalize_add_metadata(
    HirariNativeRegionAddMetadata* metadata, bool has_audio,
    uint64_t audio_samples, uint32_t audio_channels);
extern "C" bool hirari_region_move_geometry(
    const HirariNativeRegionGeometry* region, uint64_t new_start,
    HirariNativeRegionGeometry* output);
// VariAudio note-segment ownership. Rust now owns the note-edit table; C++
// keeps only a published immutable projection and the project-file layout.
typedef struct HirariRegionVariAudioStore HirariRegionVariAudioStore;

struct HirariAudioNoteSegment {
    double start_seconds = 0.0;
    double end_seconds = 0.0;
    double detected_pitch_cents = 0.0;
    double pitch_offset_cents = 0.0;
    double formant_offset_cents = 0.0;
};

struct HirariAudioNoteAnchor;
struct HirariAudioNoteCurveView;
typedef struct HirariVariAudioFrozen HirariVariAudioFrozen;

// Authoritative note-edit store. Rust-owned; C++ consumes it through this
// narrow, allocation-free handle.
extern "C" {
HirariRegionVariAudioStore* hirari_region_vari_audio_store_new();
void hirari_region_vari_audio_store_clear(HirariRegionVariAudioStore* store);
bool hirari_region_vari_audio_store_clear_segments(HirariRegionVariAudioStore* store);
// Replaces the whole table. `anchor_offsets` / `anchor_counts` describe the
// flat `anchors` layout, one entry per segment.
bool hirari_region_vari_audio_store_set(HirariRegionVariAudioStore* store,
                                        const HirariAudioNoteSegment* segments,
                                        size_t segment_count,
                                        const HirariAudioNoteAnchor* anchors,
                                        const size_t* anchor_offsets,
                                        const size_t* anchor_counts);
struct HirariAudioNoteSegmentRange;

// Control-thread snapshot of the whole table. The views it hands out point
// into the snapshot's own arrays, so the audio thread can read them without
// a lock for as long as the snapshot is retained.
HirariVariAudioFrozen* hirari_region_vari_audio_freeze(
    const HirariRegionVariAudioStore* store);
void hirari_region_vari_audio_frozen_free(HirariVariAudioFrozen* frozen);
const HirariAudioNoteSegmentRange* hirari_region_vari_audio_frozen_ranges(
    const HirariVariAudioFrozen* frozen, size_t* out_count);
const HirariAudioNoteCurveView* hirari_region_vari_audio_frozen_curves(
    const HirariVariAudioFrozen* frozen, size_t* out_count);

// Materializes the table for serialization, split and undo. Anchors are
// flattened in segment order; `anchor_offsets` / `anchor_counts` describe the
// layout. Counts are only written once every capacity check has passed.
bool hirari_region_vari_audio_store_materialize(
    const HirariRegionVariAudioStore* store,
    HirariAudioNoteSegment* out_segments, size_t segment_capacity,
    HirariAudioNoteAnchor* out_anchors, size_t anchor_capacity,
    size_t* out_anchor_offsets, size_t* out_anchor_counts,
    size_t* out_segment_count, size_t* out_anchor_total);

// Segment and anchor totals, so a caller can allocate exactly once before
// materializing.
bool hirari_region_vari_audio_store_sizes(
    const HirariRegionVariAudioStore* store, size_t* out_segment_count,
    size_t* out_anchor_total);

// Replaces the table for project load / region split / pitch analysis:
// malformed entries are repaired instead of rejecting the batch.
bool hirari_region_vari_audio_store_load(
    HirariRegionVariAudioStore* store, const HirariAudioNoteSegment* segments,
    size_t segment_count, const HirariAudioNoteAnchor* anchors,
    const size_t* anchor_offsets, const size_t* anchor_counts);

// Applies the region-add policy in place (cap, drop malformed, normalize
// anchor windows, sort, rebuild phases).
bool hirari_region_vari_audio_store_normalize(HirariRegionVariAudioStore* store);
}

// Region range edits are a pure transform: Rust reads the caller's current
// edits and returns a buffer it owns. Nothing is written through a pointer into
// caller storage, so no returned count can drive a container resize over data
// a foreign language had already written.
extern "C" HirariRegionRangeEdit* hirari_region_range_edits_apply(
    const HirariRegionRangeEdit* edits_in, size_t count_in, uint64_t region_length,
    uint32_t op, uint64_t start, uint64_t end, float gain, uint64_t fade_in,
    uint64_t fade_out, size_t* out_count);
extern "C" void hirari_free_range_edits(HirariRegionRangeEdit* edits, size_t count);
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
struct HirariEventStepView {
    uint32_t id = 0;
    float parameter = 0.0f;
    bool enabled = true;
    size_t operation_len = 0;
    size_t operation_offset = 0;
};
typedef struct HirariRegionProcessingHistoryStore HirariRegionProcessingHistoryStore;
extern "C" {
HirariRegionProcessingHistoryStore* hirari_region_processing_history_new();
void hirari_region_processing_history_clear(
    HirariRegionProcessingHistoryStore* store);
void hirari_region_processing_history_reset(
    HirariRegionProcessingHistoryStore* store);
bool hirari_region_processing_history_append(
    HirariRegionProcessingHistoryStore* store, HirariEventStep step);
size_t hirari_region_processing_history_count(
    const HirariRegionProcessingHistoryStore* store);
bool hirari_region_processing_history_sizes(
    const HirariRegionProcessingHistoryStore* store, size_t* out_count,
    size_t* out_byte_total);
bool hirari_region_processing_history_materialize(
    const HirariRegionProcessingHistoryStore* store,
    HirariEventStepView* out_views, size_t view_capacity, uint8_t* out_bytes,
    size_t byte_capacity, size_t* out_count, size_t* out_byte_total);
}
extern "C" const float* hirari_region_resampler_prepare();
struct HirariWarpMarker {
    uint64_t sourceSample = 0;
    uint64_t timelineSample = 0;
    bool transient = false;
};
// Warp-marker normalization is a pure transform: Rust reads the caller's
// markers and returns a buffer it owns, released with hirari_free_warp_markers.
extern "C" HirariWarpMarker* hirari_region_normalize_warp_markers(
    const HirariWarpMarker* markers_in, size_t count,
    uint64_t source_span, uint64_t timeline_length, size_t* out_count);
extern "C" void hirari_free_warp_markers(HirariWarpMarker* markers, size_t count);
// Region edit host bridge.
//
// Region storage stays in the engine because it owns the audio buffers and the
// immutable snapshot the audio callback reads. Rust owns the edit policy: it
// reads the current state through the host, decides, asks the host to apply,
// and asks the host to record exactly one undo entry. Every callback runs on
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

// Pre-v42 region hydration policy is Rust-owned: the source-rate migration
// (older documents stored native source frames as timeline frames) and the
// range-edit filter. Both mirror `hirari-core-bridge/src/region_hydration.rs`.
struct HirariLegacyWarpMarker {
    uint64_t source_sample = 0;
    uint64_t timeline_sample = 0;
    bool transient = false;
};

struct HirariLegacyRangeEdit {
    uint64_t start = 0;
    uint64_t end = 0;
    float gain = 1.0f;
    uint64_t fade_in = 0;
    uint64_t fade_out = 0;
};

struct HirariRegionLegacyMigration {
    uint64_t source_offset = 0;
    uint64_t base_source_offset = 0;
    uint64_t source_length = 0;
    uint64_t base_start = 0;
    double warp_ratio = 1.0;
    uint32_t loop_count = 1;
    uint64_t fade_in = 0;
    uint64_t fade_out = 0;
    double decoded_source_rate = 0.0;
    double project_sample_rate = 0.0;
    HirariLegacyWarpMarker* markers = nullptr;
    size_t marker_count = 0;
    HirariLegacyRangeEdit* range_edits = nullptr;
    size_t range_edit_count = 0;
    // Outputs, applied by the loader.
    uint64_t start = 0;
    uint64_t len = 0;
    uint64_t out_fade_in = 0;
    uint64_t out_fade_out = 0;
    double source_sample_rate = 0.0;
};

extern "C" bool hirari_region_legacy_source_rate_migrate(
    HirariRegionLegacyMigration* input);
extern "C" size_t hirari_region_range_edits_sanitize(
    HirariLegacyRangeEdit* edits, size_t count, uint64_t playable_length);

// Bounce planning is Rust-owned. C++ supplies the arrangement spans and the
// streaming writers; the acceptance rules and the render extent live in
// `hirari-core-bridge/src/bounce_plan.rs`.
struct HirariBounceRegionSpan {
    uint64_t start = 0;
    uint64_t len = 0;
    uint32_t loop_count = 1;
};

struct HirariBouncePlanInput {
    const HirariBounceRegionSpan* regions = nullptr;
    size_t region_count = 0;
    const uint64_t* midi_note_ends = nullptr;
    size_t midi_note_count = 0;
    double sample_rate = 0.0;
    uint8_t range_active = 0;
    uint64_t range_start = 0;
    uint64_t range_end = 0;
    uint32_t tail_millis = 0;
    uint64_t limiter_tail = 0;
};

struct HirariBouncePlan {
    uint64_t total_samples = 0;
    uint32_t output_rate = 0;
    uint64_t playhead_start = 0;
};

// 0 on success; -1 for an unsupported sample rate; -2 for an inverted render
// range. The engine maps each code to its existing failure path.
extern "C" int32_t hirari_bounce_render_extent(
    const HirariBouncePlanInput* input, HirariBouncePlan* out);
extern "C" bool hirari_bounce_output_path_supported(
    const uint8_t* path, size_t length, uint32_t format);
// Lifecycle admission sets (bounce_plan.rs): terminal, cancellable, pausable, resumable.
extern "C" bool hirari_bounce_state_terminal(uint32_t state), hirari_bounce_state_cancellable(uint32_t state), hirari_bounce_state_pausable(uint32_t state), hirari_bounce_state_resumable(uint32_t state);

// Formats: 0 float32 RIFF/RF64, 1 PCM16, 2 PCM24, 3 WAVE64 float. The stream
// rejects writes past the declared frame count and only publishes a complete
// file from finish(), so callers cannot emit a truncated container.
extern "C" void* hirari_bounce_stream_create(
    const uint8_t* path, size_t length, uint64_t frames, uint32_t sample_rate,
    uint8_t format);
extern "C" uint8_t hirari_bounce_stream_write(
    void* stream, const float* const* channels, uint32_t frames);
extern "C" uint8_t hirari_bounce_stream_finish(void* stream);
extern "C" void hirari_bounce_stream_free(void* stream);

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
    // Decodes a media file and returns an owned audio handle plus its rate.
    bool (*decodeAudio)(void* host, const uint8_t* path, size_t length,
                        void** outHandle, double* outRate) = nullptr;
    // Inserts a decoded region. The audio handle is consumed on every path.
    uint32_t (*insertRegion)(void* host, uint32_t trackId, size_t expectedTrack,
                             const uint8_t* path, size_t pathLength, uint64_t startSample,
                             double sourceSampleRate, double timelineSampleRate,
                             void* audioHandle) = nullptr;
    // Live track identity, or 0 when the id is not in the project.
    size_t (*trackIdentity)(void* host, uint32_t trackId) = nullptr;
    // Retains a track for a worker thread.
    void* (*retainTrack)(void* host, uint32_t trackId) = nullptr;
    void (*releaseTrack)(void* handle) = nullptr;
    bool (*pitchBegin)(void* trackHandle) = nullptr;
    bool (*pitchAnalyze)(void* trackHandle, uint32_t regionId, double sampleRate) = nullptr;
    void (*pitchFinish)(void* trackHandle) = nullptr;
    // Copies a region's note table into an owned Rust store handle.
    void* (*noteSnapshot)(void* trackHandle, uint32_t regionId) = nullptr;
    // Clones a region under a fresh id, optionally replacing its editable
    // state first, and adds the copy. Returns 0 on failure.
    uint32_t (*cloneRegion)(void* host, uint32_t trackId, uint32_t regionId,
                            const HirariRegionState* state) = nullptr;
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
extern "C" bool hirari_audio_config_valid(float tempo, double sampleRate, uint32_t blockSize);
extern "C" bool hirari_audio_format_valid(double sampleRate, uint32_t blockSize);
// Native editor session registry (native_editor_sessions.rs).
extern "C" void* hirari_native_editor_sessions_create();
extern "C" void hirari_native_editor_sessions_destroy(void* state), hirari_native_editor_sessions_clear(void* state);
extern "C" void hirari_native_editor_sessions_adopt(void* state, uint32_t track, uint32_t plugin, uint64_t session);
extern "C" bool hirari_native_editor_sessions_forget(void* state, uint32_t track, uint32_t plugin), hirari_native_editor_sessions_embedded(const void* state, uint32_t track, uint32_t plugin);
extern "C" uint64_t hirari_native_editor_sessions_session(const void* state, uint32_t track, uint32_t plugin);
extern "C" void* hirari_region_waveform(
    void* host, uint32_t trackId, uint32_t regionId, uint32_t numPeaks);
extern "C" void hirari_region_waveform_forget(void* host, uint32_t regionId);
extern "C" void hirari_region_waveform_clear(void* host);
extern "C" bool hirari_region_edit_set_warp_markers(
    void* host, uint32_t trackId, uint32_t regionId,
    const uint64_t* sourceSamples, size_t sourceCount,
    const uint64_t* timelineSamples, size_t timelineCount,
    const uint8_t* transientFlags, size_t flagCount);
// Decodes `path`, swaps it into the region and records the undo pair. Rust
// owns the path/rate policy and the snapshots; C++ performs the swap.

// Automation edit host. Rust owns the validation and the undo capture; the
// engine keeps the curves and publishes the immutable snapshots.

enum HirariAutomationEditKind : int32_t {
    kHirariAutomationVolume = 1,
    kHirariAutomationPan = 2,
    kHirariAutomationTrackDelay = 3,
    kHirariAutomationPlugin = 4,
};

struct HirariAutomationEditPayload {
    uint32_t trackId = 0;
    int32_t kind = 0;
    uint32_t pluginIndex = 0;
    uint32_t parameterId = 0;
    const Hirari::Core::ProjectAutomationPoint* points = nullptr;
    size_t pointCount = 0;
};

struct HirariAutomationHostOps {
    // Copies the target's current curve into `out` and sets `targetFound`.
    // Returns the total point count.
    size_t (*readPoints)(void* host, uint32_t trackId, int32_t kind, uint32_t pluginIndex,
                         uint32_t parameterId, Hirari::Core::ProjectAutomationPoint* out,
                         size_t capacity, bool* targetFound) = nullptr;
    bool (*apply)(void* host, const HirariAutomationEditPayload* payload) = nullptr;
    void (*recordUndo)(void* host, const char* name,
                       const HirariAutomationEditPayload* undo,
                       const HirariAutomationEditPayload* redo) = nullptr;
};

extern "C" void hirari_automation_host_install(const HirariAutomationHostOps* ops);

// Rust-side mirror of a track's region table (stage 1 of the Region storage
// migration). C++ stays authoritative and replaces the mirror on every publish.
struct HirariRegionTable;
extern "C" HirariRegionTable* hirari_region_table_new();
extern "C" void hirari_region_table_free(HirariRegionTable* table);
extern "C" bool hirari_region_table_replace(HirariRegionTable* table, const uint32_t* ids,
                                            const HirariRegionState* states, size_t count);
extern "C" size_t hirari_region_table_len(const HirariRegionTable* table);
extern "C" bool hirari_region_table_get(const HirariRegionTable* table, size_t index,
                                        uint32_t* id, HirariRegionState* state);
// Restores `store` from a snapshot handle produced by the Rust side.
extern "C" bool hirari_region_note_store_restore(void* snapshot,
                                                  HirariRegionVariAudioStore* store);

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
struct HirariTrackRegionRenderInput {
    const float* sourceLeft = nullptr;
    const float* sourceRight = nullptr;
    float* destinationLeft = nullptr;
    float* destinationRight = nullptr;
    const HirariRegionCompRange* compRanges = nullptr;
    size_t compRangeCount = 0;
    const HirariRegionRangeEdit* rangeEdits = nullptr;
    size_t rangeEditCount = 0;
    const HirariAudioNoteSegmentRange* noteRanges = nullptr;
    size_t noteRangeCount = 0;
    const HirariAudioNoteCurveView* noteCurves = nullptr;
    size_t noteCurveCount = 0;
    const HirariWarpMarker* warpMarkers = nullptr;
    size_t warpMarkerCount = 0;
    const float* resampleKernel = nullptr;
    const float* wsolaWindow = nullptr;
    void* stretchHandle = nullptr;
    uintptr_t cacheOwner = 0;
    uintptr_t cacheSnapshot = 0;
    uintptr_t cacheNoteSegments = 0;
    uint64_t playhead = 0;
    uint64_t regionStart = 0;
    uint64_t regionLength = 0;
    uint64_t loopCount = 0;
    uint64_t sourceSamples = 0;
    uint64_t sourceOffset = 0;
    uint64_t sourceLength = 0;
    uint64_t fadeIn = 0;
    uint64_t fadeOut = 0;
    uint64_t crossfadeIn = 0;
    uint64_t crossfadeOut = 0;
    double warpRatio = 1.0;
    double sourceSampleRate = 0.0;
    double timelineSampleRate = 0.0;
    double sampleRate = 44100.0;
    float clipGain = 1.0f;
    float pitchSemitones = 0.0f;
    uint32_t frameCount = 0;
    uint32_t regionId = 0;
    uint32_t syncGroup = 0;
    uint8_t muted = 0;
    uint8_t compManaged = 0;
    uint8_t reverse = 0;
    uint8_t pitchPreserveWarp = 0;
    uint8_t needsSpectralStretch = 0;
};
extern "C" bool hirari_region_needs_spectral_stretch(
    double source_rate, size_t warp_marker_count, float pitch_semitones,
    const HirariAudioNoteCurveView* note_curves, size_t note_curve_count);
// Native built-in DSP processors share one dispatch surface; the kind index
// selects the effect inside Rust.
extern "C" int32_t hirari_native_processor_kind_for_path(const uint8_t* path, size_t path_size);
extern "C" void* hirari_native_processor_create(int32_t kind);
extern "C" void hirari_native_processor_destroy(int32_t kind, void* state);
extern "C" void hirari_native_processor_prepare(int32_t kind, const void* state, double sample_rate);
extern "C" void hirari_native_processor_reset(int32_t kind, const void* state);
extern "C" void hirari_native_processor_process(
    int32_t kind, const void* state, float* const* channels, uint32_t channel_count,
    uint32_t frames, double bpm);
extern "C" void hirari_native_processor_set_parameter(
    int32_t kind, const void* state, uint32_t id, float value);
extern "C" float hirari_native_processor_get_parameter(int32_t kind, const void* state, uint32_t id);
extern "C" uint32_t hirari_native_processor_parameter_count(int32_t kind);
extern "C" uint32_t hirari_native_processor_tail_samples(int32_t kind, const void* state);
extern "C" uint32_t hirari_native_processor_latency_samples(int32_t kind, const void* state);
extern "C" const char* hirari_native_processor_name(int32_t kind);
extern "C" const char* hirari_native_processor_parameter_name(int32_t kind, uint32_t id);
// A zero return means this kind has no serializer of its own and the host
// envelope is authoritative; the four formats that do exist stay byte-exact.
extern "C" size_t hirari_native_processor_state_save(
    int32_t kind, const void* state, uint8_t bypassed, float mix,
    uint32_t sidechain_bus_id, uint8_t* output, size_t capacity);
extern "C" bool hirari_native_processor_state_restore(
    int32_t kind, void* state, const uint8_t* bytes, size_t size,
    uint8_t* bypassed, float* mix, uint32_t* sidechain_bus_id);
extern "C" void* hirari_tpdf_dither_create();
extern "C" void* hirari_tpdf_dither_create_with_seed(uint32_t seed);
extern "C" void hirari_tpdf_dither_destroy(void* state);
extern "C" float hirari_tpdf_dither_next(void* state);
extern "C" void hirari_tpdf_dither_process(void* state, float* samples, size_t frames);
extern "C" void* hirari_noise_shaping_dither_create();
extern "C" void hirari_noise_shaping_dither_destroy(void* state);
extern "C" float hirari_noise_shaping_dither_process_sample(void* state, float sample, int32_t bits);
extern "C" void hirari_noise_shaping_dither_process(
    void* state, float* samples, size_t frames, int32_t bits);
extern "C" void* hirari_master_meter_create(double sample_rate);
extern "C" void hirari_master_meter_destroy(void* state);
extern "C" void hirari_master_meter_prepare(void* state, double sample_rate);
extern "C" void hirari_master_meter_reset(void* state);
extern "C" void hirari_master_meter_process(
    void* state, const float* left, const float* right, size_t frames);
extern "C" float hirari_master_meter_get(const void* state, uint32_t field);
extern "C" void hirari_master_meter_analysis_stats(const void* state, float* output);
extern "C" float hirari_master_meter_spectrum_band(
    const void* state, uint32_t channel, uint32_t band);
extern "C" bool hirari_master_meter_goniometer(
    const void* state, float* left, float* right, size_t frames,
    float* correlation, float* balance);
extern "C" void* hirari_goniometer_create();
extern "C" void hirari_goniometer_destroy(void* state);
extern "C" void hirari_goniometer_process(
    const void* state, const float* left, const float* right, size_t frames);
extern "C" bool hirari_goniometer_snapshot(
    const void* state, float* left, float* right, size_t frames,
    float* correlation, float* balance);
extern "C" uint8_t hirari_plugin_fingerprint(const char* path, size_t path_size,
                                               uint64_t* output);

extern "C" void* hirari_plugin_registry_create();
extern "C" void hirari_plugin_registry_destroy(void* state);
extern "C" bool hirari_plugin_registry_record_scan_failure(
    void* state, const uint8_t* plugin_id, size_t plugin_id_size,
    const uint8_t* error, size_t error_size);
extern "C" bool hirari_plugin_registry_record_crash(
    void* state, const uint8_t* plugin_id, size_t plugin_id_size);
extern "C" bool hirari_plugin_registry_set_blacklisted(
    void* state, const uint8_t* plugin_id, size_t plugin_id_size,
    bool blacklisted);
extern "C" bool hirari_plugin_registry_snapshot_json(
    const void* state, uint8_t** output, size_t* output_size);
extern "C" void hirari_plugin_registry_snapshot_json_free(uint8_t* bytes, size_t size);

struct HirariWave64DecodedOwned {
    uint32_t sample_rate;
    uint16_t channels;
    uint16_t bit_depth;
    uint64_t frames;
    float* samples;
    size_t sample_count;
    uint8_t* error;
    size_t error_size;
};

extern "C" uint8_t hirari_wave64_decode(
    const char* path, size_t path_size, HirariWave64DecodedOwned* output);
extern "C" void hirari_wave64_decoded_free(HirariWave64DecodedOwned* output);
extern "C" void* hirari_wav_diagnostic_json_for_path(
    const uint8_t* path, size_t path_size, bool wave64);
extern "C" const uint8_t* hirari_wav_diagnostic_json_data(const void* value);
extern "C" size_t hirari_wav_diagnostic_json_size(const void* value);
extern "C" void hirari_wav_diagnostic_json_free(void* value);

struct HirariMpeState;
extern "C" HirariMpeState* hirari_mpe_state_create();
extern "C" void hirari_mpe_state_free(HirariMpeState* state);
extern "C" void hirari_mpe_state_reset(HirariMpeState* state);
extern "C" void hirari_midi_process_mpe(
    HirariMpeState* state, const void* events, size_t event_count);
extern "C" void hirari_midi_apply_articulations_from_state(
    const HirariMpeState* state, void* events, size_t event_count);
extern "C" void* hirari_audio_input_queue_create();
extern "C" void hirari_audio_input_queue_free(void* queue);
extern "C" bool hirari_audio_input_queue_push(
    const void* queue, const float* const* channels,
    uint32_t channel_count, uint32_t frame_count);
extern "C" bool hirari_audio_input_queue_poll(
    const void* queue, float* const* destination,
    uint32_t destination_channels, uint32_t destination_frames,
    uint32_t* channel_count, uint32_t* frame_count, uint64_t* dropped_blocks);
extern "C" uint64_t hirari_audio_input_queue_dropped(const void* queue);
extern "C" void hirari_audio_input_queue_discard(const void* queue);
extern "C" void hirari_audio_input_queue_reset(const void* queue);
// C++ mirror of Rust's NativeTempoEvent / NativeTimeSignatureEvent. These two
// plain shapes are the only form a tempo snapshot takes across the boundary,
// so the layout contract is asserted here where the offset is consumed.
struct HirariTempoEvent {
    uint64_t samplePos;
    double bpm;
    bool ramp;
    double worldBeats;
};
static_assert(std::is_standard_layout_v<HirariTempoEvent>);
static_assert(offsetof(HirariTempoEvent, samplePos) == 0 && offsetof(HirariTempoEvent, bpm) == 8 &&
              offsetof(HirariTempoEvent, ramp) == 16 &&
              offsetof(HirariTempoEvent, worldBeats) == 24 && sizeof(HirariTempoEvent) == 32);

extern "C" void* hirari_tempo_map_create();
extern "C" void hirari_tempo_map_destroy(void* state);
extern "C" bool hirari_tempo_map_add_tempo(
    void* state, uint64_t sample_pos, double bpm, double sample_rate, bool ramp);
extern "C" bool hirari_tempo_map_clear(void* state, double sample_rate, double initial_bpm);
extern "C" size_t hirari_tempo_map_get_event_count(const void* state);
extern "C" size_t hirari_tempo_map_copy_events(const void* state, void* output, size_t capacity);
extern "C" double hirari_tempo_map_samples_to_beats(
    const void* state, uint64_t samples, double sample_rate);
extern "C" uint64_t hirari_tempo_map_beats_to_samples(
    const void* state, double beats, double sample_rate);
extern "C" float hirari_tempo_map_get_current_bpm(const void* state);
extern "C" void hirari_tempo_map_set_current_bpm(void* state, float bpm);
// The engine's set_tempo entry point, implemented in Rust like every other
// tempo edit and reached only from here because a native contract test drives
// that method straight off the engine singleton.
extern "C" bool hirari_tempo_edit_set_tempo(void* tempo, void* undo, const void* log,
                                             uint64_t playhead, double sample_rate, float bpm);
// Fanout descriptors consumed by the routing engine handle. The graph, gain,
// feedback, send-PDC and topology stores are reached only through
// hirari_routing_engine_* (routing_engine_core.rs).
struct HirariRoutingTrackDestination {
    uint32_t id;
    uint8_t can_process;
    uint8_t is_bus;
    uint32_t channel_count;
    float* const* work_channels;
    void* bus_context;
};
using HirariBusPreFxAdd = bool (*)(void*, const float*, const float*, uint32_t, float);

// Routing engine composition root (hirari-core-bridge/src/routing_engine_core.rs).
// One opaque handle owns the graph/gain/feedback/send-PDC/topology stores.
extern "C" void* hirari_routing_engine_create();
extern "C" void hirari_routing_engine_destroy(void* state);
extern "C" void hirari_routing_engine_reset(void* state);
extern "C" bool hirari_routing_engine_add_connection(
    void* state, uint32_t source, uint32_t destination, float gain, bool send, bool pre_fader);
extern "C" void hirari_routing_engine_remove_connection(
    void* state, uint32_t source, uint32_t destination);
extern "C" void hirari_routing_engine_remove_send(
    void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_engine_add_dependency(
    void* state, uint32_t source, uint32_t destination);
extern "C" void hirari_routing_engine_remove_dependency(
    void* state, uint32_t source, uint32_t destination);
extern "C" void hirari_routing_engine_remove_dependencies_for_node(void* state, uint32_t node);
extern "C" void* hirari_routing_engine_remove_connections_for_node(
    void* state, uint32_t node, size_t* count);
extern "C" void* hirari_routing_engine_copy_connections(const void* state, size_t* count);
extern "C" void hirari_routing_connection_array_free(void* records, size_t count);
extern "C" void* hirari_routing_engine_remove_feedback_for_node(
    void* state, uint32_t node, size_t* count);
extern "C" void* hirari_routing_engine_copy_feedback(const void* state, size_t* count);
extern "C" void hirari_routing_feedback_array_free(void* records, size_t count);
extern "C" bool hirari_routing_engine_has_connection(
    const void* state, uint32_t source, uint32_t destination);
extern "C" float hirari_routing_engine_connection_gain(
    const void* state, uint32_t source, uint32_t destination);
extern "C" float hirari_routing_engine_send_gain(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_engine_send_pre_fader(
    const void* state, uint32_t source, uint32_t destination);
extern "C" float hirari_routing_engine_feedback_gain(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_engine_should_process(
    const void* state, uint32_t source, bool offline_target_active, uint32_t offline_target,
    bool any_solo, bool source_solo);
extern "C" bool hirari_routing_engine_add_feedback(
    void* state, uint32_t source, uint32_t destination, float gain);
extern "C" void hirari_routing_engine_remove_feedback(
    void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_engine_build(void* state);
extern "C" void hirari_routing_engine_inject_feedback(
    const void* state, uint32_t destination, float* left, float* right, uint32_t frames);
extern "C" void hirari_routing_engine_capture_feedback(
    void* state, uint32_t source, const float* left, const float* right, uint32_t frames);
extern "C" char* hirari_routing_engine_connections_json(const void* state);
extern "C" void hirari_routing_engine_connections_json_free(char* json);
extern "C" bool hirari_routing_engine_set_send_pdc(
    void* state, uint32_t source, uint32_t destination, uint32_t samples);
extern "C" void hirari_routing_engine_enter_send_pdc_read(void* state);
extern "C" void hirari_routing_engine_leave_send_pdc_read(void* state);
extern "C" bool hirari_routing_engine_resolve_process_order(
    const void* state, const uint32_t* active_ids, size_t active_count,
    uint32_t* output, size_t output_capacity, uint32_t* count_out);
extern "C" void hirari_routing_engine_process_track_fanout(
    const void* state, uint32_t source_id,
    const float* const* source_channels, uint32_t source_channel_count,
    const float* source_send_pre_left, const float* source_send_pre_right,
    const float* source_send_post_left, const float* source_send_post_right,
    uint32_t source_track_delay_samples,
    const HirariRoutingTrackDestination* destinations, size_t destination_count,
    float* master_left, float* master_right, float* send_pdc_left,
    float* send_pdc_right, uint32_t frames, uint32_t max_send_frames,
    HirariBusPreFxAdd bus_add);

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

struct HirariRoutingEdgeState {
    bool routeEnabled = false;
    float routeGain = 0.0f;
    float sendGain = 0.0f;
    bool sendPreFader = false;
    float feedbackGain = -1.0f;
};

struct HirariRoutingTrackState {
    bool armed = false;
    bool phaseInverted = false;
    bool isBus = false;
    uint32_t delaySamples = 0;
    float volume = 1.0f;
    float pan = 0.0f;
    bool muted = false;
    bool solo = false;
};

struct HirariRoutingSidechainEntry {
    uint32_t sourceId = 0;
    uint32_t destId = 0;
    uint32_t pluginIndex = 0;
    uint32_t tapPoint = 0;
};

struct HirariRoutingSidechainContext {
    uint32_t blockSize = 0;
    uint32_t sampleRate = 0;
    uint64_t generation = 0;
    bool sourceFound = false;
    bool destinationFound = false;
    bool sourcePrepared = false;
};

enum HirariRoutingEditOp : int32_t {
    kHirariRoutingEditRoute = 1,
    kHirariRoutingEditRouteGain = 2,
    kHirariRoutingEditSend = 3,
    kHirariRoutingEditFeedback = 4,
    kHirariRoutingEditSidechain = 5,
    kHirariRoutingEditArm = 6,
    kHirariRoutingEditPhase = 7,
    kHirariRoutingEditDelay = 8,
    kHirariRoutingEditVolume = 9,
    kHirariRoutingEditPan = 10,
    kHirariRoutingEditMute = 11,
    kHirariRoutingEditSolo = 12,
};

struct HirariRoutingHostOps {
    bool (*queryEdge)(void*, uint32_t, uint32_t, HirariRoutingEdgeState*) = nullptr;
    bool (*queryTrack)(void*, uint32_t, HirariRoutingTrackState*) = nullptr;
    size_t (*listSidechain)(void*, HirariRoutingSidechainEntry*, size_t) = nullptr;
    bool (*sidechainContext)(void*, uint32_t, uint32_t,
                             HirariRoutingSidechainContext*) = nullptr;
    bool (*applyRoute)(void*, const HirariRoutingEditPayload*) = nullptr;
    bool (*applyFeedback)(void*, uint32_t, uint32_t, float) = nullptr;
    bool (*applyTrackScalar)(void*, uint32_t, int32_t, bool, uint32_t) = nullptr;
    bool (*removeSidechain)(void*, uint32_t, uint32_t) = nullptr;
    bool (*registerSidechain)(void*, uint32_t, uint32_t, uint32_t,
                              const HirariRoutingSidechainContext*, uint32_t, bool) = nullptr;
    bool (*writeSidechainRoute)(void*, uint32_t, uint32_t, uint32_t, uint32_t) = nullptr;
    bool (*addDependency)(void*, uint32_t, uint32_t) = nullptr;
    bool (*removeDependency)(void*, uint32_t, uint32_t) = nullptr;
    void (*buildGraph)(void*) = nullptr;
    void (*markLayoutChanged)(void*) = nullptr;
    void (*publishRoutes)(void*) = nullptr;
    void (*recordUndo)(void*, const char*, const HirariRoutingEditPayload*, size_t,
                       const HirariRoutingEditPayload*, size_t, bool, bool) = nullptr;
    bool (*applyTrackMix)(void*, uint32_t, int32_t, float) = nullptr;
};

extern "C" void hirari_routing_edit_host_install(const HirariRoutingHostOps* ops);
extern "C" bool hirari_routing_edit_set_sidechain_link(
    void* host, uint32_t sourceId, uint32_t destId, uint32_t pluginIndex, uint32_t tapPoint,
    bool enabled);
extern "C" void hirari_routing_edit_apply_payloads(
    void* host, const HirariRoutingEditPayload* entries, size_t count);

// Control-thread edit policy (track naming, input monitoring, macro learn).
// The engine only supplies storage adapters; admission and undo live in Rust.
struct HirariControlEditHostOps {
    intptr_t (*queryTrackName)(void*, uint32_t, uint8_t*, size_t) = nullptr;
    bool (*applyTrackName)(void*, uint32_t, const uint8_t*, size_t) = nullptr;
    void (*recordNameUndo)(void*, uint32_t, const uint8_t*, size_t,
                           const uint8_t*, size_t) = nullptr;
    bool (*updateMonitorFocus)(void*, uint32_t, bool, uint32_t, uint32_t) = nullptr;
    void (*setMacroValue)(void*, uint32_t, float) = nullptr;
    bool (*bindMacro)(void*, uint32_t, uint8_t, bool, uint16_t,
                      float, float, float, bool) = nullptr;
};

extern "C" void hirari_control_edit_host_install(const HirariControlEditHostOps* ops);
extern "C" bool hirari_control_edit_set_input_monitor(
    void* host, uint32_t trackId, bool enabled, uint32_t leftChannel, uint32_t rightChannel);

// Plugin-chain edit policy (bypass, reordering, parameters and presets).
struct HirariPluginEditHostOps {
    int32_t (*queryBypass)(void*, uint32_t, uint32_t) = nullptr;
    bool (*applyBypass)(void*, uint32_t, uint32_t, bool) = nullptr;
    bool (*movePlugin)(void*, uint32_t, uint32_t, uint32_t) = nullptr;
    void (*recordBypassUndo)(void*, uint32_t, uint32_t, bool, bool) = nullptr;
    void (*recordMoveUndo)(void*, uint32_t, uint32_t, uint32_t) = nullptr;
    bool (*queryParameter)(void*, uint32_t, uint32_t, uint32_t, float*) = nullptr;
    bool (*applyParameter)(void*, uint32_t, uint32_t, uint32_t, float) = nullptr;
    void (*recordParameterUndo)(void*, uint32_t, uint32_t, uint32_t, float, float) = nullptr;
    // Reads a plugin's type and state; returns SIZE_MAX when it is missing,
    // otherwise the state length.
    size_t (*readPlugin)(void*, uint32_t, uint32_t, uint32_t*, uint8_t*, size_t) = nullptr;
    // Stores a plugin state and, when `record` is set, records the preset
    // undo entry from `before` and `state`.
    bool (*applyPluginState)(void*, uint32_t, uint32_t, const uint8_t*, size_t,
                             const uint8_t*, size_t, bool) = nullptr;
    // Appends a built-in plugin (`path` null) or a sandboxed bundle and writes
    // the new index. `prepare` re-prepares the track, as the initial insert did.
    bool (*appendPlugin)(void*, uint32_t, uint32_t, const uint8_t*, size_t, bool,
                         uint32_t*) = nullptr;
    // Removes a plugin; a non-null `outSnapshot` captures it into a handle the
    // caller must release.
    bool (*removePlugin)(void*, uint32_t, uint32_t, void**) = nullptr;
    bool (*restorePlugin)(void*, uint32_t, uint32_t, void*) = nullptr;
    void (*releasePluginSnapshot)(void*) = nullptr;
    // Records an add/remove undo entry whose undo and redo replay `edit`.
    using ReplayFn = void (*)(void*);
    void (*recordPluginReplay)(void*, const uint8_t*, size_t, void*, ReplayFn, ReplayFn,
                               ReplayFn) = nullptr;
    void (*syncStructuralChanges)(void*) = nullptr;
};

extern "C" void hirari_plugin_edit_host_install(const HirariPluginEditHostOps* ops);

// Track lifecycle edit policy (create, duplicate, remove and their undo).
struct HirariTrackEditHostOps {
    uint32_t (*allocateId)(void*) = nullptr;
    void* (*create)(void*, uint32_t, uint32_t, const uint8_t*, size_t, size_t*) = nullptr;
    void* (*duplicate)(void*, uint32_t, uint32_t*, size_t*) = nullptr;
    void* (*take)(void*, uint32_t) = nullptr;
    // The flags drop sidechain links and routing; created and removed tracks
    // take different paths. Restore replays the captured links and routes.
    bool (*remove)(void*, void*, bool, bool) = nullptr;
    bool (*restore)(void*, void*, bool) = nullptr;
    void (*releaseHandle)(void*) = nullptr;
    using ReplayFn = void (*)(void*);
    void (*recordReplay)(void*, const uint8_t*, size_t, void*, ReplayFn, ReplayFn,
                         ReplayFn) = nullptr;
    void (*syncStructuralChanges)(void*) = nullptr;
};

extern "C" void hirari_track_edit_host_install(const HirariTrackEditHostOps* ops);

// Track freeze admission policy (limits, project-end sizing, paths, ordering).
struct HirariTrackFreezeHostOps {
    // Pins a track; `wait` drains the audio callback before lookup. The
    // returned handle must be released.
    void* (*acquire)(void*, uint32_t, bool) = nullptr;
    void (*release)(void*) = nullptr;
    bool (*freeze)(void*, uint64_t, uint32_t) = nullptr;
    // Renders to the path, or restores from it when `persist` is false.
    bool (*runFromFile)(void*, const uint8_t*, size_t, uint64_t, uint32_t, bool) = nullptr;
    bool (*setCachePath)(void*, const uint8_t*, size_t) = nullptr;
    void (*unfreeze)(void*) = nullptr;
    bool (*isFrozen)(void*) = nullptr;
    // Writes the last region end; false when a region length overflows.
    bool (*projectEnd)(void*, uint64_t*) = nullptr;
    uint32_t (*blockSize)(void*) = nullptr;
};

extern "C" void hirari_track_freeze_host_install(const HirariTrackFreezeHostOps* ops);

// Control-command admission (target, generations, queue-full ordering).
struct HirariCommandHostOps {
    bool (*trackExists)(void*, uint32_t) = nullptr;
    // Writes the current project and audio-configuration generations.
    bool (*generations)(void*, uint64_t*, uint64_t*) = nullptr;
    bool (*enqueue)(void*, uint32_t, uint32_t, float, uint64_t, uint64_t) = nullptr;
    void (*journal)(void*, uint32_t, uint32_t, float, uint64_t) = nullptr;
    void (*logQueueFull)(void*, uint32_t) = nullptr;
};
extern "C" void hirari_command_host_install(const HirariCommandHostOps* ops);
extern "C" bool hirari_engine_push_command(void* host, uint32_t type, uint32_t trackId,
                                           float value, uint64_t timestamp,
                                           uint64_t expectedProjectGeneration,
                                           uint64_t expectedAudioGeneration);
extern "C" uint8_t hirari_engine_last_command_error(void* host);
extern "C" uint32_t hirari_engine_audio_command(
    uint32_t commandType, float value, float* outValue);
extern "C" uint64_t hirari_engine_chunk_plan(
    uint32_t remaining, bool playing, bool loopEnabled,
    uint64_t cycleStart, uint64_t cycleEnd, uint64_t playhead);

// Track control policy (spatial validation, HRTF admission, auto-mix curve).
struct HirariTrackControlHostOps {
    // Pins a track; the handle must be released.
    void* (*acquire)(void*, uint32_t) = nullptr;
    void (*release)(void*) = nullptr;
    void (*setSpatial)(void*, float, float, float) = nullptr;
    bool (*setHrtf)(void*, const float*, const float*, uint32_t) = nullptr;
    void (*clearHrtf)(void*) = nullptr;
};
extern "C" void hirari_track_control_host_install(const HirariTrackControlHostOps* ops);
extern "C" bool hirari_plugin_edit_set_bypass(
    void* host, uint32_t trackId, uint32_t pluginIndex, bool bypassed);
extern "C" bool hirari_plugin_edit_move(
    void* host, uint32_t trackId, uint32_t fromIndex, uint32_t toIndex);

// Status pointers refer to Track::SandboxStatus, mirrored field for field in
// plugin_sandbox_maintenance.rs.
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
extern "C" uint32_t hirari_plugin_sandbox_maintain(void* host, bool autoRestart);
extern "C" void hirari_plugin_sandbox_maintenance_forget(void* host);

extern "C" void* hirari_native_pdc_create();
extern "C" void hirari_native_pdc_destroy(void* state);
extern "C" bool hirari_native_pdc_bind_control_thread(const void* state);
extern "C" bool hirari_native_pdc_set_track_destination(
    const void* state, uint32_t track, uint32_t destination);
extern "C" bool hirari_native_pdc_set_send_route(
    const void* state, uint32_t source, uint32_t destination, bool enabled);
extern "C" bool hirari_native_pdc_set_track_latency(
    const void* state, uint32_t track, uint32_t samples);
extern "C" bool hirari_native_pdc_set_low_latency_mode(const void* state, bool active);
extern "C" bool hirari_native_pdc_clear_configuration(const void* state);
extern "C" void hirari_native_pdc_reset_for_project(const void* state);
extern "C" void hirari_native_pdc_recalculate(const void* state);
extern "C" uint32_t hirari_native_pdc_get_track_offset(const void* state, uint32_t track);
extern "C" uint32_t hirari_native_pdc_get_send_offset(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_native_pdc_low_latency_mode(const void* state);
// Track work-buffer spectrogram features: splits the first channel into
// `capacity` ranges and returns 0.5*(rms+peak) clamped to 0..1 per range.
extern "C" size_t hirari_track_analysis_spectrogram(
    const float* samples, uint32_t sample_count, float* output, size_t capacity);

extern "C" bool hirari_vocal_remove_stereo(float* left, float* right, size_t frames);
extern "C" float hirari_stereo_correlation(const float* left, const float* right, size_t frames);
extern "C" void* hirari_audio_pitch_analyze_region(
    const float* const* channels, size_t channel_count,
    size_t source_offset, size_t source_span, uint8_t reverse,
    double source_sample_rate, double timeline_sample_rate,
    uint64_t timeline_length, double source_frames_per_timeline_frame,
    const HirariWarpMarker* warp_markers, size_t warp_marker_count,
    double min_hz, double max_hz, double threshold,
    double note_change_threshold_cents, size_t note_change_confirmation_frames);
extern "C" bool hirari_audio_pitch_commit_region_result(
    const void* result, void* region_note_store);
extern "C" void hirari_audio_pitch_result_destroy(void* state);
extern "C" void* hirari_builtin_midi_instrument_create();
extern "C" void hirari_builtin_midi_instrument_destroy(void* state);
extern "C" void hirari_builtin_midi_instrument_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, uint32_t frames, double sample_rate);
extern "C" float hirari_realtime_automation_evaluate(
    const void* points, size_t count, double time, size_t* last_index_hint);
extern "C" bool hirari_vca_assign_track(uint32_t track_id, uint32_t group_id);
struct HirariRegionOverlapView {
    uint64_t start;
    uint64_t length;
    uint64_t loop_count;
    uint8_t muted;
    uint64_t crossfade_in;
    uint64_t crossfade_out;
};
extern "C" bool hirari_regions_apply_overlap_crossfades(
    HirariRegionOverlapView* regions, size_t region_count);
struct HirariGeneratedMidiEvent {
    uint64_t sample_offset;
    uint8_t articulation_id;
    uint8_t size;
    uint8_t data[3];
};
struct HirariTimedKeySwitchOutput {
    uint8_t channel;
    uint8_t pitch;
    uint32_t length_ticks;
};
extern "C" void* hirari_track_midi_articulation_create();
extern "C" void hirari_track_midi_articulation_destroy(void* state);
extern "C" bool hirari_track_midi_expression_map_set(
    const void* state, uint8_t map_kind, const uint8_t* data, size_t length);
extern "C" uint8_t* hirari_track_midi_expression_map_take(
    const void* state, uint8_t map_kind, size_t* size);
extern "C" void hirari_track_midi_expression_map_take_free(uint8_t*, size_t);
extern "C" size_t hirari_track_midi_timed_outputs(
    const void* state, uint8_t articulation_id, bool transitions,
    HirariTimedKeySwitchOutput* output, size_t capacity);
extern "C" bool hirari_track_midi_schedule_note_off(
    const void* state, uint8_t channel, uint8_t pitch, uint64_t sample);
extern "C" size_t hirari_track_midi_note_on(
    const void* state, uint8_t channel, uint8_t pitch, uint8_t velocity,
    uint64_t sample_offset, uint8_t articulation_id,
    HirariGeneratedMidiEvent* output, size_t capacity);
extern "C" size_t hirari_track_midi_note_off(
    const void* state, uint8_t channel, uint8_t pitch, uint64_t sample_offset,
    uint8_t articulation_id, HirariGeneratedMidiEvent* output, size_t capacity);
extern "C" size_t hirari_track_midi_all_notes_off(
    const void* state, uint64_t sample_offset,
    HirariGeneratedMidiEvent* output, size_t capacity);
extern "C" size_t hirari_track_midi_flush_scheduled_note_offs(
    const void* state, uint64_t playhead, uint32_t block_size,
    HirariGeneratedMidiEvent* output, size_t capacity);
struct HirariTrackAutomationCapturePoint {
    double time;
    float volume;
    float pan;
};
extern "C" void* hirari_automation_recorder_instance();
extern "C" uint32_t hirari_automation_record_mode_apply(uint32_t mode);
extern "C" void* hirari_track_automation_capture_create();
extern "C" void hirari_track_automation_capture_destroy(void* state);
extern "C" bool hirari_track_automation_capture_record(
    const void* state, const void* recorder, uint32_t track_id,
    uint64_t sample_position, float volume, float pan);
extern "C" HirariTrackAutomationCapturePoint* hirari_track_automation_capture_drain(
    const void* state, size_t* drained);
extern "C" void hirari_track_automation_capture_points_free(
    HirariTrackAutomationCapturePoint* points, size_t count);
extern "C" float hirari_track_eq_gain(float boost_db, float cut_db);
extern "C" void* hirari_track_pdc_delay_create();
extern "C" void hirari_track_pdc_delay_destroy(void* state);
extern "C" void hirari_track_pdc_delay_reset(void* state, uint32_t requested);
extern "C" void hirari_track_finalize_output(
    uint32_t track_id, uint32_t frames, double start_time,
    const void* volume_points, size_t volume_count, size_t* volume_hint,
    const void* pan_points, size_t pan_count, size_t* pan_hint,
    const void* channel_strip,
    float* const* active_channels, uint32_t active_channel_count,
    float* const* pre_insert_channels, uint32_t pre_insert_channel_count,
    float* const* send_pre_channels, uint32_t send_pre_channel_count,
    float* const* send_post_channels, uint32_t send_post_channel_count,
    const void* panner, uint32_t spatial_mode, float spatial_x, float spatial_y,
    float spatial_z, float low_gain, float high_gain,
    float* low_state_l, float* low_state_r, void* pdc_delay,
    uint32_t requested_pdc, uint32_t manual_delay,
    float* const* pre_fader_channels, uint32_t pre_fader_channel_count);
extern "C" void* hirari_track_holographic_panner_create();
extern "C" void hirari_track_holographic_panner_destroy(void* state);
extern "C" void hirari_track_holographic_panner_set_sample_rate(const void* state, double sample_rate);
extern "C" double hirari_track_holographic_panner_sample_rate(const void* state);
extern "C" bool hirari_track_holographic_panner_set_kernel(
    const void* state, const float* left, const float* right, uint32_t taps);
extern "C" void hirari_track_holographic_panner_clear_kernel(const void* state);
extern "C" void hirari_track_holographic_panner_process(
    const void* state, float* left, float* right, uint32_t frames, float x, float y, float z);
using HirariEffectNodeInfo = bool (*)(void*, uint32_t, bool*, bool*);
using HirariEffectNodeProcess = bool (*)(void*, uint32_t, bool, float*);
using HirariEffectNodeProcessWithHandle = bool (*)(
    void*, uint32_t, void*, bool, float*);
struct HirariEffectChainNodeView {
    void* processor;
    uint8_t bypassed;
    uint8_t parallel;
    uint32_t latency_samples;
};
static_assert(sizeof(HirariEffectChainNodeView) == 16);
extern "C" void* hirari_effect_chain_runtime_create();
extern "C" bool hirari_effect_chain_runtime_enter_audio(const void* state);
extern "C" void hirari_effect_chain_runtime_leave_audio(const void* state);
extern "C" void hirari_effect_chain_runtime_begin_mutation(const void* state);
extern "C" void hirari_effect_chain_runtime_end_mutation(const void* state);
extern "C" uint32_t hirari_effect_chain_runtime_audio_reader_count(const void* state);
extern "C" void hirari_effect_chain_runtime_destroy(void* state);
extern "C" bool hirari_effect_chain_runtime_publish(
    void* state, const HirariEffectChainNodeView* nodes, size_t count);
extern "C" void hirari_effect_chain_runtime_reclaim(const void* state);
extern "C" void* hirari_effect_chain_runtime_processor_at(
    const void* state, uint32_t index);
using HirariEffectChainMetricValue = uint64_t (*)(void*, uint32_t);
extern "C" uint32_t hirari_effect_chain_runtime_total_latency_samples(const void* state);
extern "C" uint64_t hirari_effect_chain_runtime_metric(
    const void* state, uint32_t metric, HirariEffectChainMetricValue read_value);
extern "C" void hirari_effect_chain_runtime_process_block(
    const void* state, void* user_data,
    float* const* channels, uint32_t channel_count,
    float* const* parallel_channels, uint32_t parallel_channel_count,
    uint32_t parallel_capacity, uint32_t frames,
    HirariEffectNodeProcessWithHandle process_node);
struct HirariPluginAutomationLaneView {
    uint32_t processor_index;
    uint32_t parameter_id;
    const void* points;
    size_t point_count;
};
static_assert(sizeof(HirariPluginAutomationLaneView) == 24,
    "plugin automation lane view must match Rust's repr(C) layout");
using HirariTrackMidiPrepare = void (*)(void*, uint64_t, uint32_t);
struct HirariTrackProcessRequest {
    float* const* channels;
    uint32_t channel_count;
    uint32_t buffer_capacity;
    uint32_t frames;
    uint64_t playhead;
    uint8_t clear_input;
    uint8_t muted;
    uint8_t frozen;
    const float* const* frozen_channels;
    uint32_t frozen_channel_count;
    uint32_t frozen_capacity;
    uint64_t frozen_total_samples;
    const HirariTrackRegionRenderInput* region_inputs;
    size_t region_count;
    uint8_t region_snapshot_available;
    uint8_t phase_invert;
    const void* track_delay_points;
    size_t track_delay_count;
    size_t* track_delay_hint;
    uint32_t* track_delay_output;
    uint32_t max_track_delay_samples;
    const HirariPluginAutomationLaneView* plugin_lanes;
    size_t plugin_lane_count;
    uint8_t transport_playing;
    void* plugin_events;
    size_t plugin_event_capacity;
    size_t* plugin_event_count_out;
    float* const* pre_insert_channels;
    uint32_t pre_insert_channel_count;
    uint32_t pre_insert_capacity;
    HirariTrackMidiPrepare midi_prepare;
    void* midi_context;
};
static_assert(sizeof(HirariTrackProcessRequest) == 208,
    "track block process request must match Rust's repr(C) layout");
extern "C" bool hirari_track_process_prepare(const HirariTrackProcessRequest* request);

using HirariTrackFreezeResetCallback = bool (*)(void* context);
using HirariTrackFreezeProcessCallback = bool (*)(void* context, float* left, float* right,
                                                   uint32_t frames, uint64_t playhead);
extern "C" bool hirari_track_freeze_render(
    uint64_t total_samples, uint32_t block_size,
    float* output_left, float* output_right, size_t output_capacity,
    void* context, HirariTrackFreezeResetCallback reset,
    HirariTrackFreezeProcessCallback process);
extern "C" bool hirari_track_freeze_write_float32(
    const uint8_t* path, size_t path_size, uint32_t sample_rate,
    const float* left, const float* right, uint64_t total_samples);
extern "C" bool hirari_audio_quantize_group(
    const float* const* inputs, float* const* outputs, uint32_t channels,
    uint64_t length, float bpm, double sample_rate, float strength, float swing);
extern "C" void* hirari_midi_region_state_create(const void* notes, size_t count);
extern "C" void hirari_midi_region_state_destroy(void* state);
extern "C" size_t hirari_midi_region_state_count(const void* state);
extern "C" size_t hirari_midi_region_state_copy(
    const void* state, void* output, size_t capacity);
extern "C" bool hirari_midi_region_state_add(void* state, const void* note);
extern "C" bool hirari_midi_region_state_remove(void* state, size_t index);
extern "C" bool hirari_midi_region_state_update(void* state, size_t index, const void* note);
extern "C" bool hirari_midi_region_state_replace(void* state, const void* notes, size_t count);
extern "C" void hirari_midi_region_state_transpose(
    void* state, int32_t semitones, double selection_start, double selection_end);
extern "C" bool hirari_midi_region_state_quantize(
    void* state, double grid, double strength, double selection_start, double selection_end);
extern "C" void hirari_midi_region_state_remove_notes_at(
    void* state, double beat, int32_t pitch, double tolerance);
extern "C" void hirari_midi_region_state_set_muted_at(
    void* state, double beat, int32_t pitch, bool muted);
extern "C" void* hirari_midi_fragment_reassembler_create();
extern "C" void hirari_midi_fragment_reassembler_destroy(void* state);
extern "C" void hirari_midi_fragment_reassembler_reset(void* state);
extern "C" uint8_t hirari_midi_fragment_reassembler_push(
    void* state, uint32_t message_id, uint16_t index, uint16_t total,
    uint64_t sample_offset, uint8_t articulation_id, const uint8_t* bytes, uint16_t size);
extern "C" bool hirari_midi_fragment_reassembler_complete(const void* state);
extern "C" size_t hirari_midi_fragment_reassembler_size(const void* state);
extern "C" uint32_t hirari_midi_fragment_reassembler_message_id(const void* state);
extern "C" uint64_t hirari_midi_fragment_reassembler_sample_offset(const void* state);
extern "C" uint8_t hirari_midi_fragment_reassembler_articulation_id(const void* state);
extern "C" const uint8_t* hirari_midi_fragment_reassembler_data(const void* state);
extern "C" bool hirari_midi_extended_ring_init(void* storage, size_t bytes);
extern "C" bool hirari_midi_extended_ring_push(
    void* state, uint64_t sample_offset, uint8_t articulation_id,
    const uint8_t* bytes, size_t size);
extern "C" bool hirari_midi_extended_ring_pop(
    void* state, void* destination_message, size_t destination_bytes);
extern "C" size_t hirari_midi_extended_ring_size(const void* state);
extern "C" void* hirari_realtime_memory_pool_default_allocate(size_t size_bytes);
extern "C" void hirari_forensic_scratchpad_reset();
extern "C" bool hirari_status_queue_push_default(
    uint32_t severity, const uint8_t* text, size_t text_size);
extern "C" bool hirari_status_queue_pop_default(void* output_message);
extern "C" uint64_t hirari_status_queue_take_dropped_default();
extern "C" void* hirari_audio_scheduler_create();
extern "C" void hirari_audio_scheduler_destroy(void* state);
extern "C" bool hirari_audio_scheduler_start(
    const void* state, size_t workers, size_t queue_capacity,
    void (*dispatch)(void*), uint64_t (*enter)(uint32_t), void (*leave)(uint64_t));
extern "C" void hirari_audio_scheduler_stop(const void* state);
extern "C" size_t hirari_audio_scheduler_thread_count(const void* state);
extern "C" bool hirari_audio_scheduler_is_running(const void* state);
extern "C" bool hirari_audio_scheduler_push(const void* state, size_t worker, void* data);
extern "C" bool hirari_audio_scheduler_steal(const void* state, size_t worker, void** data);
extern "C" void hirari_audio_scheduler_parallel_for(
    const void* state, uint32_t start, uint32_t end, void* context,
    void (*callback)(uint32_t, void*));
extern "C" void* hirari_metronome_create(double sample_rate);
extern "C" void hirari_metronome_destroy(void* state);
extern "C" void hirari_metronome_set_enabled(void* state, bool enabled);
extern "C" bool hirari_metronome_is_enabled(const void* state);
extern "C" void hirari_metronome_process(
    void* state, float* left, float* right, uint32_t sample_count,
    uint64_t playhead, double sample_rate, double bpm);
extern "C" void* hirari_undo_manager_create(size_t max_depth, uint64_t coalesce_window_ms);
extern "C" uint64_t hirari_undo_manager_timestamp_ms();
extern "C" uint64_t hirari_undo_manager_register_native_callback(
    void* state, void* context, void (*invoke)(void*), void (*destroy)(void*));
extern "C" bool hirari_undo_manager_invoke_native_callback(void* state, uint64_t callback_id);
extern "C" void hirari_undo_manager_discard_native_callback(void* state, uint64_t callback_id);
extern "C" bool hirari_undo_manager_apply_and_invoke(void* state, bool redo);
extern "C" bool hirari_undo_manager_abort_and_invoke(void* state);
extern "C" void hirari_undo_manager_destroy(void* state);
extern "C" bool hirari_undo_manager_record(
    void* state, const uint8_t* name, size_t name_len,
    uint64_t undo_callback, uint64_t redo_callback, uint64_t timestamp_ms, bool coalesce);
extern "C" bool hirari_undo_manager_begin_transaction(
    void* state, const uint8_t* name, size_t name_len);
extern "C" bool hirari_undo_manager_end_transaction(void* state);
extern "C" bool hirari_undo_manager_transaction_active(const void* state);
extern "C" size_t hirari_undo_manager_undo_count(const void* state);
extern "C" size_t hirari_undo_manager_redo_count(const void* state);
extern "C" size_t hirari_undo_manager_top_name(
    const void* state, bool redo, uint8_t* output, size_t capacity);
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
extern "C" uint8_t hirari_plugin_admission_error(const char* path);
extern "C" const char* hirari_plugin_scan_rejection(const char* path);
extern "C" const char* hirari_internal_plugin_path(uint32_t plugin_type, bool bus);
extern "C" size_t hirari_plugin_sandbox_find(const uint32_t*, size_t, uint32_t);
extern "C" size_t hirari_plugin_sandbox_remove(uint32_t*, uint32_t*, size_t, uint32_t);
extern "C" const char* hirari_plugin_sandbox_failure_name(uint8_t code), *hirari_plugin_sandbox_state_error_name(uint8_t code);
// Built-in effect parameter names; kind matches the Rust-builtin effect table.
extern "C" const char* hirari_builtin_effect_parameter_name(uint32_t kind, uint32_t id);
extern "C" uint32_t hirari_plugin_sandbox_block_outcome(
    bool produced, uint32_t overruns, uint8_t failure_code, bool failed);
extern "C" uint32_t hirari_plugin_sandbox_transport_retry_delay(uint32_t attempt, uint8_t failure);
extern "C" size_t hirari_plugin_sandbox_move(uint32_t*, uint32_t*, size_t, size_t, uint32_t, uint32_t);
extern "C" void* hirari_midi_learn_create();
extern "C" void hirari_midi_learn_destroy(void* state);
extern "C" void hirari_midi_learn_add_mapping(
    void* state, uint8_t cc, uint8_t channel, const void* macro_state,
    uint32_t macro_index,
    float minimum, float maximum, float curve, bool pickup);
extern "C" void hirari_midi_learn_add_mapping_14bit(
    void* state, uint16_t controller, uint8_t channel, const void* macro_state,
    uint32_t macro_index,
    float minimum, float maximum, float curve, bool pickup);
extern "C" void hirari_midi_learn_handle_cc(
    const void* state, uint8_t channel, uint8_t cc, uint8_t value);
extern "C" void hirari_midi_learn_handle_cc14(
    const void* state, uint8_t channel, uint16_t controller, uint16_t value);

extern "C" void* hirari_project_db_create();
extern "C" void hirari_project_db_destroy(void* state);
extern "C" bool hirari_project_db_record(
    void* state, uint64_t timestamp,
    const uint8_t* category, size_t category_size,
    const uint8_t* key, size_t key_size,
    const uint8_t* value, size_t value_size);
using HirariProjectDbCallback = void (*)(
    void*, uint64_t, const uint8_t*, size_t,
    const uint8_t*, size_t, const uint8_t*, size_t);
extern "C" bool hirari_project_db_query(
    const void* state, const uint8_t* category, size_t category_size,
    void* context, HirariProjectDbCallback callback);
extern "C" bool hirari_project_db_flush(
    const void* state, const uint8_t* path, size_t path_size);
extern "C" void* hirari_mpe_manager_create();
extern "C" void hirari_mpe_manager_destroy(void* state);
extern "C" void hirari_mpe_manager_reset(void* state);

using HirariForensicJournalRecoveryCallback = void (*)(
    void*, uint32_t, uint32_t, float, uint64_t);
extern "C" void* hirari_forensic_journaler_create();
extern "C" void hirari_forensic_journaler_destroy(void* state);
extern "C" bool hirari_forensic_journaler_start(
    void* state, const uint8_t* path, size_t path_size);
extern "C" bool hirari_forensic_journaler_log(
    const void* state, uint32_t type, uint32_t track_id, float value, uint64_t timestamp);
extern "C" bool hirari_forensic_journaler_log_description(
    const void* state, uint32_t type, const uint8_t* description,
    size_t description_size, float value);
extern "C" void hirari_forensic_journaler_stop(void* state);
extern "C" bool hirari_forensic_journaler_recover(
    const uint8_t* path, size_t path_size, void* context,
    HirariForensicJournalRecoveryCallback callback);

using HirariSnapshotComputeCallback = bool (*)(void*, void**);
using HirariSnapshotReadCallback = bool (*)(void*, uint64_t*, uint64_t*, const uint8_t**, size_t*);
using HirariSnapshotDestroyCallback = void (*)(void*);
using HirariSnapshotCancelCallback = void (*)(void*);
using HirariSnapshotIsCancelledCallback = bool (*)(const void*);
using HirariSnapshotBytesVisitor = void (*)(void*, const uint8_t*, size_t);
using HirariWaveformComputeCallback = size_t (*)(void*, uint32_t, uint32_t, float*, size_t);
using HirariWaveformVisitor = void (*)(void*, const float*, size_t);
extern "C" uint64_t hirari_layout_snapshot_request(
    uint64_t owner, void* context, HirariSnapshotComputeCallback compute, HirariSnapshotReadCallback read,
    HirariSnapshotDestroyCallback destroy_result, HirariSnapshotDestroyCallback destroy_context,
    HirariSnapshotCancelCallback set_cancelled, HirariSnapshotIsCancelledCallback is_cancelled);
extern "C" uint8_t hirari_layout_snapshot_poll(
    uint64_t id, uint64_t owner, void* context, HirariSnapshotBytesVisitor visit);
extern "C" bool hirari_layout_snapshot_cancel(uint64_t id, uint64_t owner);
extern "C" uint64_t hirari_layout_snapshot_owner_create();
extern "C" void hirari_layout_snapshot_owner_release(uint64_t owner);
extern "C" uint64_t hirari_waveform_snapshot_request(
    void* context, uint32_t track_id, uint32_t region_id,
    HirariWaveformComputeCallback compute, HirariSnapshotDestroyCallback destroy_context);
extern "C" bool hirari_waveform_snapshot_poll(
    uint64_t id, void* context, HirariWaveformVisitor visit);
extern "C" bool hirari_waveform_snapshot_pending(uint64_t id);
extern "C" uint8_t hirari_wav_write_planar(
    const uint8_t* path, size_t path_size, const float* const* channels,
    size_t channel_count, uint64_t frames, uint32_t sample_rate, uint8_t format);
using HirariVideoDecodeCallback = bool (*)(void*, double, uint64_t, void**);
using HirariVideoReadFrameCallback = bool (*)(
    void*, uint32_t*, uint32_t*, double*, const uint8_t**, size_t*);
using HirariVideoDestroyFrameCallback = void (*)(void*);
using HirariVideoPixelsVisitor = void (*)(void*, const uint8_t*, size_t);
extern "C" void* hirari_video_runtime_create(
    void* context, HirariVideoDecodeCallback decode,
    HirariVideoReadFrameCallback read, HirariVideoDestroyFrameCallback destroy);
extern "C" void hirari_video_runtime_destroy(void* runtime);
extern "C" uint64_t hirari_video_runtime_clear(void* runtime);
extern "C" bool hirari_video_runtime_set_metadata(void* runtime, double duration, double fps);
extern "C" void hirari_video_runtime_update(const void* runtime, uint64_t sample, double sample_rate);
extern "C" bool hirari_video_runtime_request_frame(void* runtime, double seconds);
extern "C" bool hirari_video_runtime_has_video(const void* runtime);
extern "C" double hirari_video_runtime_fps(const void* runtime);
extern "C" double hirari_video_runtime_duration(const void* runtime);
extern "C" double hirari_video_runtime_requested_seconds(const void* runtime);
extern "C" uint64_t hirari_video_runtime_revision(const void* runtime);
extern "C" bool hirari_video_runtime_copy_frame(
    const void* runtime, void* context, HirariVideoPixelsVisitor visit,
    uint32_t* width, uint32_t* height, double* seconds);

// Project-layout snapshot consumed by the Rust serializer. Every pointer is
// borrowed only for the duration of the synchronous write call; Rust never
// stores or frees any of it. The returned JSON buffer is Rust-owned and must
// be released with hirari_project_layout_json_free.
struct HirariLayoutString {
    const uint8_t* data = nullptr;
    size_t len = 0;
};
struct HirariLayoutAutomationPoint {
    double time = 0.0;
    float value = 0.0f;
    float curve = 0.0f;
};
struct HirariLayoutPluginLane {
    uint32_t pluginIndex = 0;
    uint32_t parameterId = 0;
    const HirariLayoutAutomationPoint* points = nullptr;
    size_t pointCount = 0;
};
struct HirariLayoutParameterBlock {
    const float* values = nullptr;
    size_t count = 0;
};
struct HirariLayoutByteBlock {
    const uint8_t* bytes = nullptr;
    size_t count = 0;
};
struct HirariLayoutSidechainRoute {
    uint32_t sourceId = 0;
    uint32_t destinationId = 0;
    uint32_t pluginIndex = 0;
    uint32_t tapPoint = 0;
};
struct HirariLayoutFeedbackRoute {
    uint32_t sourceId = 0;
    uint32_t destinationId = 0;
    float gain = 0.0f;
};
struct HirariLayoutRegion {
    uint32_t id = 0;
    HirariLayoutString name;
    HirariRegionState state;
    uint32_t sourceSampleRate = 0;
    HirariLayoutString path;
    bool missing = false;
    bool audioLoaded = false;
    const HirariWarpMarker* warpMarkers = nullptr;
    size_t warpMarkerCount = 0;
    const HirariRegionRangeEdit* rangeEdits = nullptr;
    size_t rangeEditCount = 0;
    const HirariRegionVariAudioStore* noteStore = nullptr;
    // Immutable projection preferred by the serializer when the region has
    // been published; the live store may be mutated by a concurrent edit.
    const HirariVariAudioFrozen* noteFrozen = nullptr;
    const HirariRegionProcessingHistoryStore* processingStore = nullptr;
};
struct HirariLayoutTrack {
    uint32_t id = 0;
    HirariLayoutString name;
    uint32_t type = 0;
    float volume = 0.0f;
    float pan = 0.0f;
    bool muted = false;
    bool solo = false;
    bool recordArmed = false;
    bool phaseInverted = false;
    uint64_t trackDelaySamples = 0;
    bool frozen = false;
    uint64_t frozenTotalSamples = 0;
    uint32_t frozenSampleRate = 0;
    HirariLayoutString frozenPath;
    const HirariLayoutAutomationPoint* volumeAutomation = nullptr;
    size_t volumeAutomationCount = 0;
    const HirariLayoutAutomationPoint* panAutomation = nullptr;
    size_t panAutomationCount = 0;
    const HirariLayoutAutomationPoint* trackDelayAutomation = nullptr;
    size_t trackDelayAutomationCount = 0;
    HirariLayoutString expressionMapJson;
    HirariLayoutString expressionMapProJson;
    const HirariLayoutPluginLane* pluginAutomation = nullptr;
    size_t pluginAutomationCount = 0;
    const uint32_t* pluginTypes = nullptr;
    size_t pluginTypeCount = 0;
    const uint8_t* pluginBypass = nullptr;  // one byte per plugin
    const HirariLayoutParameterBlock* pluginParameters = nullptr;  // one per plugin
    const HirariLayoutByteBlock* pluginStates = nullptr;
    size_t pluginStateCount = 0;
    const HirariLayoutString* sandboxPluginPaths = nullptr;
    size_t sandboxPluginPathCount = 0;
    const HirariLayoutByteBlock* sandboxPluginStates = nullptr;
    size_t sandboxPluginStateCount = 0;
    const HirariLayoutRegion* regions = nullptr;
    size_t regionCount = 0;
};
struct HirariProjectLayoutSnapshot {
    const HirariLayoutTrack* tracks = nullptr;
    size_t trackCount = 0;
    const HirariLayoutSidechainRoute* sidechainRoutes = nullptr;
    size_t sidechainRouteCount = 0;
    const HirariLayoutFeedbackRoute* feedbackRoutes = nullptr;
    size_t feedbackRouteCount = 0;
    double timelineSampleRate = 0.0;
};
using HirariLayoutIsCancelledCallback = bool (*)(const void*);
extern "C" bool hirari_project_layout_write_json(
    const HirariProjectLayoutSnapshot* snapshot,
    HirariLayoutIsCancelledCallback is_cancelled, const void* cancel_context,
    const uint8_t** json_data, size_t* json_size);
extern "C" void hirari_project_layout_json_free(const uint8_t* json_data, size_t json_size);

// Rust-owned harmonic context shared by structure analysis and auto-arrangement.
extern "C" void hirari_harmonic_context_get(
    float* tension, float* valence, float* stability);
