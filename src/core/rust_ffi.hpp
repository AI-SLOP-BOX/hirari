#pragma once

#include <cstddef>
#include <cstdint>

extern "C" void* hirari_virtuoso_drum_synth_create(double sample_rate);
extern "C" void hirari_virtuoso_drum_synth_destroy(void* state);
extern "C" void hirari_virtuoso_drum_synth_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_drum_synth_reset(void* state);
extern "C" void hirari_virtuoso_drum_synth_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, size_t frames);
extern "C" void* hirari_noise_gate_create(double sample_rate);
extern "C" void hirari_noise_gate_destroy(void* state);
extern "C" void hirari_noise_gate_prepare(void* state, double sample_rate);
extern "C" void hirari_noise_gate_reset(void* state);
extern "C" void hirari_noise_gate_process(
    void* state, float* left, float* right,
    const float* sidechain_left, const float* sidechain_right,
    size_t frames);
extern "C" void* hirari_virtuoso_vocal_create(double sample_rate);
extern "C" void hirari_virtuoso_vocal_destroy(void* state);
extern "C" void hirari_virtuoso_vocal_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_vocal_reset(void* state);
extern "C" void hirari_virtuoso_vocal_set_pitch(void* state, float semitones);
extern "C" void hirari_virtuoso_vocal_set_formant(void* state, float semitones);
extern "C" void hirari_virtuoso_vocal_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_virtuoso_pitch_create(double sample_rate);
extern "C" void hirari_virtuoso_pitch_destroy(void* state);
extern "C" void hirari_virtuoso_pitch_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_pitch_reset(void* state);
extern "C" void hirari_virtuoso_pitch_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_virtuoso_stradivari_create(double sample_rate);
extern "C" void hirari_virtuoso_stradivari_destroy(void* state);
extern "C" void hirari_virtuoso_stradivari_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_stradivari_reset(void* state);
extern "C" void hirari_virtuoso_stradivari_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, size_t frames);

struct HirariMidiScoreGlyph {
    float beat;
    float staff_offset;
    int32_t duration;
    uint32_t source_region_id;
    uint32_t source_note_index;
};
extern "C" size_t hirari_notation_midi_manifest(
    const double* region_starts, const double* region_lengths,
    const uint32_t* region_ids, const double* note_starts,
    const double* note_lengths, const uint8_t* pitches,
    const uint8_t* velocities, const uint32_t* note_indices,
    size_t count, HirariMidiScoreGlyph* output, size_t output_capacity);
extern "C" size_t hirari_notation_audio_manifest(
    const uint64_t* starts, const uint64_t* lengths, const uint8_t* muted,
    const double* sample_rates, size_t count,
    HirariMidiScoreGlyph* output, size_t output_capacity);
extern "C" void* hirari_log_buffer_init(void* storage, size_t bytes);
extern "C" uint64_t hirari_log_buffer_timestamp();
extern "C" void hirari_log_buffer_post(
    const void* state, uint32_t level, uint32_t component_id,
    const uint8_t* message, size_t message_len);
extern "C" bool hirari_log_buffer_pop(const void* state, void* output);

// Narrow C ABI for native compatibility callers. Algorithms and policy stay
// in Rust; this header only describes the byte-oriented boundary.
extern "C" uint32_t hirari_crc32_bytes(const uint8_t* data, size_t size);
struct HirariAestheticFeatures {
    float spectral_balance;
    float dynamic_complexity;
    float transient_clarity;
    float stereo_width;
    float phase_coherence;
};
extern "C" void hirari_aesthetic_analyze(
    const float* left, const float* right, uint32_t samples,
    HirariAestheticFeatures* output);
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
extern "C" int64_t hirari_midi_edit_range(
    HirariScheduledMidiNote* notes, size_t count, uint32_t track_id,
    uint64_t start_sample, uint64_t end_sample, uint32_t operation, int64_t amount);
using HirariScheduledMidiEventCallback = void (*)(
    void* context, const HirariScheduledMidiNote* note,
    uint64_t sample_offset, uint8_t note_off);
extern "C" void hirari_midi_schedule_block(
    const HirariScheduledMidiNote* notes, size_t note_count,
    const size_t* indices_by_end, size_t end_count, uint64_t playhead,
    uint32_t frames, uint8_t playback_active, uint8_t reconcile,
    uint64_t pass, void* context, HirariScheduledMidiEventCallback callback);
extern "C" void* hirari_midi_input_buffer_create();
extern "C" void hirari_midi_input_buffer_destroy(void* state);
extern "C" void hirari_midi_input_buffer_push(
    const void* state, uint8_t status, uint8_t data1, uint8_t data2, uint64_t timestamp);
extern "C" bool hirari_midi_input_buffer_add_blacklist_rule(
    const void* state, uint8_t status_mask, uint8_t status_value, uint8_t data1, uint8_t data2);
extern "C" void hirari_midi_input_buffer_clear_blacklist(const void* state);
extern "C" uint64_t hirari_midi_input_buffer_dropped(const void* state);
extern "C" size_t hirari_midi_input_buffer_pull(
    const void* state, void* output, size_t capacity);
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
extern "C" void* hirari_tap_tempo_create(size_t max_taps);
extern "C" void hirari_tap_tempo_destroy(void* state);
extern "C" void hirari_tap_tempo_tap(void* state, uint64_t timestamp_ms);
extern "C" void hirari_tap_tempo_clear(void* state);
extern "C" double hirari_tap_tempo_bpm(const void* state);
extern "C" size_t hirari_tap_tempo_count(const void* state);
extern "C" void* hirari_cabinet_simulator_create();
extern "C" void hirari_cabinet_simulator_destroy(void* state);
extern "C" void hirari_cabinet_simulator_reset(void* state);
extern "C" void hirari_cabinet_simulator_set_model(void* state, uint32_t model);
extern "C" void hirari_cabinet_simulator_process(
    void* state, float* left, float* right, uint32_t frames);
extern "C" void* hirari_stereo_imager_create(double sample_rate);
extern "C" void hirari_stereo_imager_destroy(void* state);
extern "C" void hirari_stereo_imager_reset(void* state);
extern "C" void hirari_stereo_imager_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_stereo_imager_set_width(void* state, float width);
extern "C" float hirari_stereo_imager_get_width(const void* state);
extern "C" void hirari_stereo_imager_process(
    void* state, float* left, float* right, uint32_t frames, float mix);
extern "C" void* hirari_lush_reverb_create(double sample_rate);
extern "C" void hirari_lush_reverb_destroy(void* state);
extern "C" void hirari_lush_reverb_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_lush_reverb_reset(void* state);
extern "C" void hirari_lush_reverb_process(
    void* state, float* left, float* right, size_t frames);
extern "C" uint32_t hirari_lush_reverb_tail(const void* state);
extern "C" void* hirari_stereo_expander_create();
extern "C" void hirari_stereo_expander_destroy(void* state);
extern "C" void hirari_stereo_expander_set_params(void* state, float width, float mid_gain);
extern "C" void hirari_stereo_expander_set_parameter(void* state, uint32_t parameter, float value);
extern "C" float hirari_stereo_expander_get_parameter(const void* state, uint32_t parameter);
extern "C" void hirari_stereo_expander_process(
    void* state, float* left, float* right, uint32_t frames, float mix);
extern "C" void* hirari_atmos_reverb_create(double sample_rate);
extern "C" void hirari_atmos_reverb_destroy(void* state);
extern "C" void hirari_atmos_reverb_prepare(void* state, double sample_rate);
extern "C" void hirari_atmos_reverb_reset(void* state);
extern "C" uint32_t hirari_atmos_reverb_tail_samples(const void* state);
extern "C" void hirari_atmos_reverb_process(
    void* state, float* const* buffers, uint32_t buffer_count, uint32_t frames);
extern "C" void* hirari_deesser_create(double sample_rate);
extern "C" void hirari_deesser_destroy(void* state);
extern "C" void hirari_deesser_prepare(void* state, double sample_rate);
extern "C" void hirari_deesser_reset(void* state);
extern "C" void hirari_deesser_set_threshold(void* state, float value);
extern "C" void hirari_deesser_set_intensity(void* state, float value);
extern "C" uint32_t hirari_deesser_tail_samples(const void* state);
extern "C" void hirari_deesser_process(
    void* state, float* left, float* right, uint32_t frames);
extern "C" void* hirari_transient_shaper_create(double sample_rate);
extern "C" void hirari_transient_shaper_destroy(void* state);
extern "C" void hirari_transient_shaper_prepare(void* state, double sample_rate);
extern "C" void hirari_transient_shaper_reset(void* state);
extern "C" void hirari_transient_shaper_set_attack(void* state, float value);
extern "C" void hirari_transient_shaper_set_sustain(void* state, float value);
extern "C" void hirari_transient_shaper_process(
    void* state, float* left, float* right, uint32_t frames);
extern "C" void* hirari_chromaglow_create(double sample_rate);
extern "C" void hirari_chromaglow_destroy(void* state);
extern "C" void hirari_chromaglow_prepare(void* state, double sample_rate);
extern "C" void hirari_chromaglow_reset(void* state);
extern "C" void hirari_chromaglow_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_chromaglow_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_chromaglow_set_params(
    void* state, float drive_db, float character, uint32_t mode);
extern "C" void hirari_chromaglow_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_chromaglow_process(
    void* state, float* left, float* right, uint32_t frames);
extern "C" void* hirari_waveform_cache_map_create(
    const char* path, const float* samples, size_t sample_count);
extern "C" const float* hirari_waveform_cache_map_data(const void* cache);
extern "C" size_t hirari_waveform_cache_map_len(const void* cache);
extern "C" void hirari_waveform_cache_map_destroy(void* cache);
using HirariWaveformSampleCount = bool (*)(void*, uint64_t*);
using HirariWaveformSample = bool (*)(void*, uint64_t, float*);
extern "C" void* hirari_waveform_overview_create(
    void* source, HirariWaveformSampleCount sample_count, HirariWaveformSample sample);
extern "C" void hirari_waveform_overview_destroy(void* state);
extern "C" bool hirari_waveform_overview_failed(const void* state);
extern "C" bool hirari_waveform_overview_wait(const void* state, uint64_t timeout_ms);
extern "C" uint32_t hirari_waveform_overview_lod_count(const void* state);
extern "C" bool hirari_waveform_overview_lod_info(
    const void* state, uint32_t index, uint32_t* ratio, size_t* peak_count);
extern "C" bool hirari_waveform_overview_copy_lod(
    const void* state, uint32_t index, float* min, float* max, size_t capacity);
extern "C" size_t hirari_waveform_overview_copy_error(
    const void* state, uint8_t* destination, size_t capacity);
extern "C" void* hirari_mapped_audio_file_open(const char* path);
extern "C" void hirari_mapped_audio_file_destroy(void* file);
extern "C" bool hirari_mapped_audio_file_is_valid(const void* file);
extern "C" float hirari_mapped_audio_file_sample(const void* file, uint32_t channel, uint64_t frame);
extern "C" uint64_t hirari_mapped_audio_file_frames(const void* file);
extern "C" uint32_t hirari_mapped_audio_file_channels(const void* file);
extern "C" uint32_t hirari_mapped_audio_file_sample_rate(const void* file);
extern "C" size_t hirari_mapped_audio_file_size(const void* file);
extern "C" const void* hirari_mapped_audio_file_data(const void* file);
extern "C" bool hirari_mapped_audio_file_refresh(const void* file);
extern "C" void* hirari_audio_pool_peaks_create(const void* mapped_file);
extern "C" void hirari_audio_pool_peaks_destroy(void* hierarchy);
extern "C" size_t hirari_audio_pool_peaks_level_count(const void* hierarchy);
extern "C" uint32_t hirari_audio_pool_peaks_level_step(const void* hierarchy, size_t level);
extern "C" size_t hirari_audio_pool_peaks_level_len(const void* hierarchy, size_t level);
extern "C" const float* hirari_audio_pool_peaks_level_min(const void* hierarchy, size_t level);
extern "C" const float* hirari_audio_pool_peaks_level_max(const void* hierarchy, size_t level);
extern "C" void hirari_audio_buffer_clear(
    float* const* channels, uint32_t channel_count, uint32_t offset, uint32_t sample_count);
extern "C" uint32_t hirari_audio_buffer_sanitize_non_finite(
    float* const* channels, uint32_t channel_count, uint32_t sample_count);
extern "C" void hirari_audio_buffer_add_channels(
    float* const* destinations, const float* const* sources,
    uint32_t channel_count, uint32_t sample_count);
extern "C" void hirari_audio_route_gain_correction(
    float* destination_left, float* destination_right,
    const float* source_left, const float* source_right,
    uint32_t frames, float gain);
extern "C" void hirari_audio_add_sanitized_stereo(
    float* destination_left, float* destination_right,
    const float* source_left, const float* source_right, uint32_t frames);
extern "C" bool hirari_audio_copy_monitor_input(
    const float* const* inputs, uint32_t channel_count,
    uint32_t left_channel, uint32_t right_channel, uint32_t frames,
    float* output_left, float* output_right);
extern "C" bool hirari_audio_copy_talkback_input(
    const float* const* inputs, uint32_t channel_count, uint32_t input_channel,
    uint32_t frames, float* output);
extern "C" void hirari_audio_buffer_apply_gain(
    float* const* channels, uint32_t channel_count, uint32_t sample_count, float gain);
extern "C" uint32_t hirari_builtin_gain_process(
    float* const* channels, uint32_t channel_count, uint32_t sample_count, float gain);
extern "C" void* hirari_builtin_gain_create();
extern "C" void hirari_builtin_gain_destroy(void* state);
extern "C" void hirari_builtin_gain_prepare(
    const void* state, double sample_rate, uint32_t block_size);
extern "C" void hirari_builtin_gain_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_builtin_gain_get_parameter(const void* state, uint32_t id);
extern "C" uint32_t hirari_builtin_gain_process_state(
    const void* state, float* const* channels, uint32_t channel_count,
    uint32_t sample_count);
extern "C" void* hirari_preview_synth_create();
extern "C" void hirari_preview_synth_destroy(void* state);
extern "C" void hirari_preview_synth_set_engine(void* state, uint32_t engine);
extern "C" void hirari_preview_synth_reset(void* state);
extern "C" void hirari_preview_synth_process(
    void* state, const void* events, size_t event_count, float* left, float* right,
    uint32_t frames, uint64_t playhead, double sample_rate, float morph,
    double detune_ratio, float drive, float cutoff, float resonance, float output_gain);
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
extern "C" void hirari_control_room_process_monitor(
    float* left, float* right, const float* talkback, uint32_t frames,
    float monitor_gain, float talkback_gain, bool talkback_enabled);
extern "C" void hirari_control_room_mix_cue(
    const float* source_left, const float* source_right,
    const float* click_left, const float* click_right,
    float* output_left, float* output_right, uint32_t frames,
    float cue_gain, bool click_enabled);
struct HirariControlRoomSpeakerSnapshot {
    uint8_t name[128];
    uint32_t name_length;
    float gain;
    uint8_t enabled;
};
struct HirariControlRoomCueSnapshot {
    uint32_t id;
    float gain;
    uint8_t enabled;
    uint32_t bus_track_id;
    uint32_t output_channel;
    uint8_t click_enabled;
};
extern "C" void* hirari_control_room_state_create();
extern "C" void hirari_control_room_state_destroy(void* state);
extern "C" void hirari_control_room_reset(void* state);
extern "C" bool hirari_control_room_add_speaker(
    void* state, const uint8_t* name, size_t name_length, float gain);
extern "C" bool hirari_control_room_select_speaker(void* state, size_t index);
extern "C" bool hirari_control_room_remove_speaker(void* state, size_t index);
extern "C" bool hirari_control_room_rename_speaker(
    void* state, size_t index, const uint8_t* name, size_t name_length);
extern "C" bool hirari_control_room_set_speaker_gain(void* state, size_t index, float gain);
extern "C" bool hirari_control_room_set_speaker_enabled(void* state, size_t index, bool enabled);
extern "C" void hirari_control_room_set_dim(void* state, bool enabled);
extern "C" bool hirari_control_room_set_dim_db(void* state, float db);
extern "C" float hirari_control_room_dim_db(void* state);
extern "C" bool hirari_control_room_is_dimmed(void* state);
extern "C" void hirari_control_room_set_talkback(void* state, bool enabled, float gain);
extern "C" bool hirari_control_room_talkback_enabled(const void* state);
extern "C" bool hirari_control_room_set_talkback_channel(void* state, uint32_t channel);
extern "C" uint32_t hirari_control_room_talkback_channel(const void* state);
extern "C" float hirari_control_room_monitor_gain(const void* state);
extern "C" void hirari_control_room_process_monitor_state(
    const void* state, float* left, float* right, const float* talkback, uint32_t frames);
extern "C" bool hirari_control_room_upsert_cue(void* state, uint32_t id, float gain, bool enabled);
extern "C" bool hirari_control_room_remove_cue(void* state, uint32_t id);
extern "C" bool hirari_control_room_set_cue_enabled(void* state, uint32_t id, bool enabled);
extern "C" bool hirari_control_room_set_cue_bus(void* state, uint32_t id, uint32_t bus_track_id);
extern "C" bool hirari_control_room_set_cue_output(void* state, uint32_t id, uint32_t output_channel);
extern "C" bool hirari_control_room_set_cue_click(void* state, uint32_t id, bool enabled);
extern "C" bool hirari_control_room_select_cue(void* state, uint32_t id);
extern "C" uint32_t hirari_control_room_active_cue_id(const void* state);
extern "C" uint32_t hirari_control_room_active_cue_bus(const void* state);
extern "C" uint32_t hirari_control_room_cue_bus(void* state, uint32_t id);
extern "C" float hirari_control_room_active_cue_gain(const void* state);
extern "C" uint32_t hirari_control_room_active_cue_output(const void* state);
extern "C" bool hirari_control_room_active_cue_click(const void* state);
extern "C" bool hirari_control_room_is_cue_bus(const void* state, uint32_t id);
extern "C" float hirari_control_room_cue_gain(void* state, uint32_t id);
extern "C" bool hirari_control_room_validate(void* state);
extern "C" size_t hirari_control_room_active_speaker(void* state);
extern "C" size_t hirari_control_room_speaker_snapshot(
    void* state, HirariControlRoomSpeakerSnapshot* output, size_t capacity);
extern "C" size_t hirari_control_room_cue_snapshot(
    void* state, HirariControlRoomCueSnapshot* output, size_t capacity);
extern "C" bool hirari_bus_accumulate_stereo(
    float* bus_left, float* bus_right, const float* input_left,
    const float* input_right, uint32_t frames, float gain);
extern "C" void* hirari_bus_audio_create();
extern "C" void hirari_bus_audio_destroy(void* state);
extern "C" bool hirari_bus_audio_accumulate(
    void* state, const float* input_left, const float* input_right,
    uint32_t frames, float gain);
extern "C" bool hirari_bus_audio_replace_post(
    void* state, const float* left, const float* right, uint32_t frames);
extern "C" void hirari_bus_audio_clear(void* state, uint32_t frames);
extern "C" bool hirari_bus_audio_commit_dry(void* state, uint32_t frames);
extern "C" bool hirari_bus_audio_read(
    void* state, float* left, float* right, uint32_t frames, bool post);
extern "C" uint32_t hirari_bus_audio_samples(const void* state);
extern "C" void* hirari_bus_track_create();
extern "C" void hirari_bus_track_destroy(void* state);
extern "C" void hirari_bus_track_set_input_gain(void* state, float gain);
extern "C" void hirari_bus_track_set_phase_inverted(void* state, bool inverted);
extern "C" void hirari_bus_track_set_read_post_fx(void* state, bool post_fx);
extern "C" bool hirari_bus_track_fetch_audio(
    const void* track_state, void* bus_audio_state,
    float* left, float* right, uint32_t frames);
extern "C" uint64_t hirari_plugin_state_checksum(const uint8_t* data, size_t size);
extern "C" uint8_t hirari_plugin_state_validate(
    const uint8_t* data, size_t size, uint32_t version, uint64_t expected_checksum);
extern "C" void* hirari_plugin_parameter_snapshot_create();
extern "C" void hirari_plugin_parameter_snapshot_destroy(void* handle);
extern "C" bool hirari_plugin_parameter_snapshot_set(const void* handle, uint32_t parameter_id, double value);
extern "C" bool hirari_plugin_parameter_snapshot_get(const void* handle, uint32_t parameter_id, double* output);
using HirariPluginParameterVisitCallback = bool (*)(void* context, uint32_t parameter_id, double value);
extern "C" bool hirari_plugin_parameter_snapshot_visit(
    const void* handle, void* context, HirariPluginParameterVisitCallback callback);
extern "C" void* hirari_bus_system_create();
extern "C" void hirari_bus_system_destroy(void* state);
extern "C" bool hirari_bus_system_bind(void* state, uint32_t bus_id, void* bus_audio_state);
extern "C" bool hirari_bus_system_register(void* state, uint32_t bus_id);
extern "C" bool hirari_bus_system_reconcile(
    void* state, const bool* active, uint32_t count);
extern "C" void hirari_bus_system_reset(void* state);
extern "C" bool hirari_bus_system_is_present(const void* state, uint32_t bus_id);
extern "C" bool hirari_bus_system_update_routing(
    void* state, const uint32_t* ids, size_t count);
extern "C" void hirari_bus_system_process(const void* state, uint32_t frames);
extern "C" void hirari_bus_system_clear(const void* state, uint32_t frames);
extern "C" bool hirari_audio_buffer_copy(
    float* destination_left, float* destination_right,
    const float* source_left, const float* source_right, uint32_t sample_count);
extern "C" float hirari_audio_buffer_magnitude(const float* samples, uint32_t sample_count);
extern "C" bool hirari_stem_split_process(
    const float* const* inputs,
    float* const* drums,
    float* const* bass,
    float* const* vocals,
    float* const* other,
    uint32_t channels,
    uint32_t samples,
    double sample_rate);
extern "C" void* hirari_audio_buffer_storage_create();
extern "C" void hirari_audio_buffer_storage_destroy(void* storage);
extern "C" bool hirari_audio_buffer_storage_reserve(void* storage, size_t capacity);
extern "C" void hirari_audio_buffer_storage_release(void* storage);
extern "C" float* hirari_audio_buffer_storage_data(const void* storage);
extern "C" size_t hirari_audio_buffer_storage_capacity(const void* storage);
extern "C" bool hirari_audio_buffer_storage_prepare_channels(void* storage, size_t channel_count);
extern "C" void hirari_audio_buffer_storage_set_channel_pointer(
    void* storage, size_t channel, float* pointer);
extern "C" float** hirari_audio_buffer_storage_channel_pointers(const void* storage);
extern "C" size_t hirari_audio_buffer_storage_channel_count(const void* storage);
extern "C" bool hirari_audio_buffer_deinterleave_interleaved(
    const float* source, size_t source_sample_count,
    float* const* channel_pointers, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_parameter_smoother_create(float initial);
extern "C" void hirari_parameter_smoother_destroy(void* state);
extern "C" void hirari_parameter_smoother_set_target(const void* state, float value);
extern "C" void hirari_parameter_smoother_reset(const void* state, float value);
extern "C" void hirari_parameter_smoother_set_time(
    const void* state, float milliseconds, float sample_rate);
extern "C" void hirari_parameter_smoother_process(
    const void* state, float* buffer, uint32_t length);
extern "C" float hirari_parameter_smoother_next(const void* state);
extern "C" float hirari_parameter_smoother_current(const void* state);
extern "C" void* hirari_macro_mapping_create();
extern "C" void hirari_macro_mapping_destroy(void* state);
extern "C" void hirari_macro_mapping_add(
    void* state, uint32_t macro_index, uint32_t target_id,
    float minimum, float maximum, bool invert);
extern "C" void hirari_macro_mapping_clear(void* state, uint32_t macro_index);
extern "C" void hirari_macro_control_set_value(
    void* state, uint32_t macro_index, float value);
extern "C" float hirari_macro_control_get_value(
    const void* state, uint32_t macro_index);
extern "C" void hirari_macro_control_set_midi_target(
    const void* state, uint32_t macro_index, float value);
extern "C" void hirari_macro_control_update_smoothers(
    const void* state, float sample_rate);
extern "C" float hirari_macro_mapping_evaluate(
    const void* state, uint32_t macro_index, uint32_t target_id);
extern "C" void* hirari_linear_ramp_smoother_create();
extern "C" void* hirari_linear_ramp_smoother_clone(const void* state);
extern "C" void hirari_linear_ramp_smoother_destroy(void* state);
extern "C" void hirari_linear_ramp_smoother_reset(void* state, double sample_rate, double time_ms);
extern "C" void hirari_linear_ramp_smoother_set_target(void* state, float target);
extern "C" float hirari_linear_ramp_smoother_next(void* state);
extern "C" void hirari_linear_ramp_smoother_skip(void* state, uint32_t samples);
extern "C" float hirari_linear_ramp_smoother_current(const void* state);
extern "C" void* hirari_automation_curve_create();
extern "C" void hirari_automation_curve_destroy(void* state);
extern "C" void hirari_automation_curve_add_point(
    void* state, double time, float value, int32_t interpolation);
extern "C" float hirari_automation_curve_value_at(const void* state, double time);
extern "C" size_t hirari_automation_curve_copy_points(
    const void* state, void* output, size_t capacity);
extern "C" void* hirari_param_tree_create();
extern "C" void hirari_param_tree_destroy(void* tree);
extern "C" bool hirari_param_tree_set(const void* tree, uint32_t id, float value);
extern "C" float hirari_param_tree_get(const void* tree, uint32_t id, float fallback);
extern "C" void hirari_param_tree_clear(const void* tree);
extern "C" void* hirari_ahdsr_create(double sample_rate);
extern "C" void hirari_ahdsr_destroy(void* envelope);
extern "C" void hirari_ahdsr_set_parameters(
    void* envelope, float attack, float hold, float decay, float sustain, float release);
extern "C" void hirari_ahdsr_reset(void* envelope);
extern "C" void hirari_ahdsr_trigger(void* envelope);
extern "C" void hirari_ahdsr_release(void* envelope);
extern "C" float hirari_ahdsr_next_value(void* envelope);
extern "C" bool hirari_ahdsr_is_active(const void* envelope);
extern "C" void* hirari_atomic_parameter_create(float initial, uint8_t display_mode);
extern "C" void hirari_atomic_parameter_destroy(void* state);
extern "C" float hirari_atomic_parameter_get_target(const void* state);
extern "C" void hirari_atomic_parameter_set_target(const void* state, float value);
extern "C" float hirari_atomic_parameter_get_next(const void* state);
extern "C" void hirari_atomic_parameter_get_block(
    const void* state, float* buffer, size_t length);
extern "C" float hirari_atomic_parameter_get_normalized(const void* state);
extern "C" void hirari_atomic_parameter_set_unit(const void* state, uint8_t unit);
extern "C" void hirari_atomic_parameter_set_display_mode(const void* state, uint8_t mode);
extern "C" void hirari_atomic_parameter_set_sample_rate(const void* state, double sample_rate);
extern "C" void hirari_atomic_parameter_set_smoothing_time(
    const void* state, double milliseconds);
extern "C" void hirari_atomic_parameter_set_ai_modulation(const void* state, float offset);
extern "C" void hirari_atomic_parameter_reset_to_target(const void* state);
extern "C" void hirari_atomic_parameter_set_reset_value(const void* state, float value);
extern "C" void hirari_atomic_parameter_set_smoothing_type(const void* state, uint8_t type);
extern "C" void hirari_atomic_parameter_get_value_string(
    const void* state, char* buffer, size_t size);
extern "C" void* hirari_console_strip_create(double sample_rate);
extern "C" void hirari_console_strip_destroy(void* state);
extern "C" void hirari_console_strip_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_console_strip_set_input_gain(void* state, float gain);
extern "C" void hirari_console_strip_set_output_gain(void* state, float gain);
extern "C" void hirari_console_strip_set_threshold(void* state, float threshold_db);
extern "C" bool hirari_console_strip_set_band(
    void* state, uint32_t index, float frequency, float gain_db, float q);
extern "C" void hirari_console_strip_reset(void* state);
extern "C" void hirari_console_strip_process(
    void* state, float** channels, uint32_t channel_count, size_t frames);
extern "C" void* hirari_auto_filter_create();
extern "C" void hirari_auto_filter_destroy(void* state);
extern "C" void hirari_auto_filter_set_sample_rate(const void* state, double sample_rate);
extern "C" void hirari_auto_filter_reset(const void* state);
extern "C" void hirari_auto_filter_set_parameter(const void* state, uint32_t id, float value);
extern "C" float hirari_auto_filter_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_auto_filter_process(
    const void* state, float** channels, uint32_t channel_count, size_t frames, float mix);
extern "C" void* hirari_multiband_exciter_create(double sample_rate);
extern "C" void hirari_multiband_exciter_destroy(void* state);
extern "C" void hirari_multiband_exciter_set_sample_rate(
    const void* state, double sample_rate);
extern "C" void hirari_multiband_exciter_setup_crossover(
    const void* state, float low_cut, float high_cut);
extern "C" void hirari_multiband_exciter_reset(const void* state);
extern "C" void hirari_multiband_exciter_process(
    const void* state, float* left, float* right, uint32_t frames);
extern "C" void* hirari_multiband_compressor_create(double sample_rate);
extern "C" void hirari_multiband_compressor_destroy(void* state);
extern "C" void hirari_multiband_compressor_prepare(void* state, double sample_rate);
extern "C" void hirari_multiband_compressor_reset(void* state);
extern "C" void hirari_multiband_compressor_set_split_freqs(
    void* state, float low_mid_hz, float mid_high_hz);
extern "C" void hirari_multiband_compressor_process(
    void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" uint32_t hirari_multiband_compressor_tail(const void* state);
extern "C" void* hirari_sidechain_compressor_create(double sample_rate);
extern "C" void hirari_sidechain_compressor_destroy(void* state);
extern "C" void hirari_sidechain_compressor_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_sidechain_compressor_set_control(
    const void* state, uint32_t control, float value);
extern "C" float hirari_sidechain_compressor_get_parameter(
    const void* state, uint32_t parameter);
extern "C" void hirari_sidechain_compressor_set_parameter(
    const void* state, uint32_t parameter, float value);
extern "C" uint32_t hirari_sidechain_compressor_tail(const void* state);
extern "C" size_t hirari_sidechain_compressor_write_state(
    const void* state, uint8_t* output, size_t capacity, bool bypassed,
    float mix, uint32_t sidechain);
extern "C" bool hirari_sidechain_compressor_restore_state(
    const void* state, const uint8_t* input, size_t length, bool* bypassed,
    float* mix, uint32_t* sidechain);
extern "C" void hirari_sidechain_compressor_reset(void* state);
extern "C" void hirari_sidechain_compressor_process(
    void* state, const float* main_left, const float* main_right,
    float* output_left, float* output_right,
    const float* sidechain_left, const float* sidechain_right,
    size_t sidechain_frames, size_t frames, double sample_rate);
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
extern "C" void hirari_midi_buffer_sort(void* events, size_t count);
extern "C" void* hirari_midi_buffer_create();
extern "C" void hirari_midi_buffer_destroy(void* state);
extern "C" void hirari_midi_buffer_add(
    void* state, uint64_t sample_offset, const uint8_t* data, uint32_t size,
    uint8_t articulation_id);
struct HirariMixerTelemetrySnapshot {
    float peak_l;
    float peak_r;
    float rms_l;
    float rms_r;
    uint32_t clipping_count;
    float dc_offset_l;
    float dc_offset_r;
    float phase_correlation;
    float loudness_lufs;
    float spectrum_rms[8];
};
extern "C" void* hirari_mixer_telemetry_create();
extern "C" void hirari_mixer_telemetry_destroy(void* state);
extern "C" void hirari_mixer_telemetry_push(
    const void* state, uint32_t track_id, const float* left, const float* right,
    uint32_t frames);
extern "C" bool hirari_mixer_telemetry_read(
    const void* state, uint32_t track_id, HirariMixerTelemetrySnapshot* output);
extern "C" void* hirari_step_sequencer_create();
extern "C" void hirari_step_sequencer_destroy(void* state);
extern "C" void hirari_step_sequencer_set_swing(const void* state, float amount);
extern "C" void hirari_step_sequencer_set_active(const void* state, bool active);
extern "C" bool hirari_step_sequencer_get_step(const void* state, uint32_t lane, uint32_t step);
extern "C" void hirari_step_sequencer_set_step(
    const void* state, uint32_t lane, uint32_t step, bool active);
extern "C" void hirari_step_sequencer_set_probability(
    const void* state, uint32_t lane, uint32_t step, uint32_t probability);
extern "C" void hirari_step_sequencer_set_substeps(
    const void* state, uint32_t lane, uint32_t step, uint32_t count);
extern "C" void hirari_step_sequencer_set_offset(
    const void* state, uint32_t lane, uint32_t step, float offset);
extern "C" void hirari_step_sequencer_process(
    const void* state, void* midi_state, uint64_t current_position,
    uint32_t num_samples, double bpm, double sample_rate);
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
extern "C" bool hirari_midi_buffer_chord_trigger(
    void* input, void* scratch, const int32_t* intervals,
    size_t interval_count, uint32_t strum_samples);
extern "C" void* hirari_chord_trigger_create();
extern "C" void hirari_chord_trigger_destroy(void* state);
extern "C" void hirari_chord_trigger_prepare(void* state, double sample_rate);
extern "C" void hirari_chord_trigger_set_strum_ms(void* state, float value);
extern "C" float hirari_chord_trigger_get_strum_ms(const void* state);
extern "C" bool hirari_chord_trigger_save_state(
    const void* state, bool bypassed, float mix, uint32_t sidechain_bus_id,
    uint8_t* output, size_t capacity);
extern "C" bool hirari_chord_trigger_restore_state(
    void* state, const uint8_t* input, size_t length, bool* bypassed,
    float* mix, uint32_t* sidechain_bus_id);
extern "C" void hirari_chord_trigger_reset(void* state);
extern "C" bool hirari_chord_trigger_process(void* state, void* input_midi_state);
extern "C" uint8_t hirari_midi_buffer_add_event(void* events, size_t capacity,
    size_t* count, uint64_t sample_offset, const uint8_t* data, uint32_t size,
    uint8_t articulation_id);
extern "C" uint8_t hirari_midi_buffer_copy_event(void* events, size_t capacity,
    size_t* count, const void* source);
extern "C" void* hirari_arpeggiator_create(double sample_rate);
extern "C" void hirari_arpeggiator_destroy(void* state);
extern "C" void hirari_arpeggiator_set_mode(void* state, uint32_t mode);
extern "C" uint32_t hirari_arpeggiator_get_mode(const void* state);
extern "C" void hirari_arpeggiator_prepare(void* state, double sample_rate);
extern "C" void hirari_arpeggiator_reset(void* state);
extern "C" size_t hirari_arpeggiator_process(
    void* state, const void* events, size_t count, double bpm,
    double sample_rate, uint64_t block_start, uint32_t num_samples);
extern "C" const void* hirari_arpeggiator_output(const void* state);
extern "C" bool hirari_neural_advice_evaluate(
    float lufs_integrated, float true_peak, uint64_t timestamp);
extern "C" bool hirari_neural_advice_pop(void* output);
extern "C" void hirari_fade_apply(
    float* output, const float* input_a, const float* input_b, size_t frames);
extern "C" void hirari_fade_apply_micro(float* buffer, size_t frames, bool fade_in);
extern "C" float hirari_fade_factor(
    size_t position, size_t length, bool fade_in, uint8_t curve, float curvature);
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
extern "C" bool hirari_region_set_warp_ratio_geometry(
    const HirariNativeRegionGeometry* region, double ratio,
    HirariNativeRegionGeometry* output);
extern "C" bool hirari_region_set_trim_geometry(
    const HirariNativeRegionGeometry* region, float start_normalized,
    float end_normalized, HirariNativeRegionGeometry* output);
extern "C" bool hirari_region_move_geometry(
    const HirariNativeRegionGeometry* region, uint64_t new_start,
    HirariNativeRegionGeometry* output);
extern "C" size_t hirari_region_normalize_range_edits(
    HirariRegionRangeEdit* edits, size_t count, uint64_t region_length);
extern "C" size_t hirari_region_range_edit_upsert(
    HirariRegionRangeEdit* edits, size_t count, size_t capacity,
    uint64_t region_length, uint64_t start, uint64_t end, float gain,
    uint64_t fade_in, uint64_t fade_out);
extern "C" size_t hirari_region_range_edit_remove(
    HirariRegionRangeEdit* edits, size_t count, uint64_t start, uint64_t end);
extern "C" bool hirari_region_range_edits_replace(
    HirariRegionRangeEdit* edits, size_t count, uint64_t region_length);
struct HirariRegionCompRange {
    uint64_t start;
    uint64_t end;
    uint64_t fade_in;
    uint64_t fade_out;
};
extern "C" void hirari_region_gain_block(
    float* output, uint32_t frames, uint64_t region_offset,
    uint64_t region_length, uint64_t timeline_length,
    uint64_t fade_in, uint64_t fade_out, uint64_t crossfade_in,
    uint64_t crossfade_out, float clip_gain, uint8_t comp_managed,
    const HirariRegionCompRange* comp_ranges, size_t comp_range_count,
    const HirariRegionRangeEdit* range_edits, size_t range_edit_count);
extern "C" const float* hirari_region_resampler_prepare();
extern "C" void hirari_region_read_warped(
    const float* source, uint64_t source_samples, uint64_t source_offset,
    uint64_t source_span, double position, uint8_t reverse,
    uint8_t allow_source_preroll, double resample_step, const float* kernel,
    float* output_value_and_slope);
extern "C" bool hirari_region_pitch_correction_delays(
    double sample_rate, double reference_pitch_cents,
    double* output_minimum_range_and_reference_hz);
extern "C" bool hirari_region_pitch_corrected_frame(
    const float* source_left, const float* source_right,
    uint64_t source_samples, uint64_t source_offset, uint64_t source_span,
    double warped_position, double local_source_rate, double effective_pitch_ratio,
    double base_pitch_ratio, uint64_t loop_relative, double note_correction_seconds,
    double sample_rate, double minimum_delay, double delay_range, uint8_t reverse,
    const float* kernel, float* output);
struct HirariWarpMarker {
    uint64_t sourceSample = 0;
    uint64_t timelineSample = 0;
    bool transient = false;
};
extern "C" size_t hirari_region_normalize_warp_markers(
    HirariWarpMarker* markers, size_t count,
    uint64_t source_span, uint64_t timeline_length);
extern "C" bool hirari_region_align_audio(
    const float* const* reference_channels, size_t reference_channel_count,
    size_t reference_offset, size_t reference_span,
    const float* const* target_channels, size_t target_channel_count,
    size_t target_offset, size_t target_span, uint64_t target_timeline_length,
    HirariWarpMarker* output_markers, size_t output_capacity, size_t* output_count);
extern "C" bool hirari_region_sync_move_positions(
    const uint64_t* starts, const uint64_t* base_starts, size_t count,
    uint64_t anchor_start, uint64_t new_anchor_start,
    uint64_t* output_starts, uint64_t* output_base_starts);
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
extern "C" double hirari_region_source_position_at(
    const HirariWarpMarker* markers, size_t marker_count,
    uint64_t timeline_sample, double source_rate, uint64_t source_span);
extern "C" void hirari_region_source_block(
    const HirariWarpMarker* markers, size_t marker_count,
    uint64_t first_region_sample, uint64_t region_length,
    double source_rate, uint64_t source_span, uint32_t frame_count,
    double* positions_output, double* rates_output);
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
struct HirariAudioNoteSegmentValidation {
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double detectedPitchCents = 0.0;
    double pitchOffsetCents = 0.0;
    double formantOffsetCents = 0.0;
    const HirariAudioNoteAnchor* anchors = nullptr;
    size_t anchorCount = 0;
};
struct HirariAudioNotePhaseView {
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double pitchOffsetCents = 0.0;
    const HirariAudioNoteAnchor* anchors = nullptr;
    size_t anchorCount = 0;
    const double* integralPrefix = nullptr;
};
extern "C" bool hirari_region_pitch_corrected_frame_with_curves(
    const float* source_left, const float* source_right,
    uint64_t source_samples, uint64_t source_offset, uint64_t source_span,
    double warped_position, double local_source_rate, double effective_pitch_ratio,
    double base_pitch_ratio, uint64_t loop_relative, double note_seconds,
    double sample_rate, double minimum_delay, double delay_range, uint8_t reverse,
    const float* kernel, const HirariAudioNoteCurveView* matched_curve,
    const HirariAudioNoteCurveView* previous_curve, float* output);
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
extern "C" bool hirari_track_render_region(const HirariTrackRegionRenderInput* input);
extern "C" size_t hirari_track_render_regions(
    const HirariTrackRegionRenderInput* inputs, size_t input_count,
    uint64_t playhead, uint32_t frame_count,
    float* destination_left, float* destination_right, uint8_t phase_invert);
extern "C" uint32_t hirari_audio_buffer_render_frozen(
    float* const* destinations, uint32_t destination_channels,
    uint32_t destination_samples, const float* const* sources,
    uint32_t source_channels, uint32_t source_samples,
    uint64_t total_samples, uint64_t playhead, uint32_t frame_count);
extern "C" void hirari_audio_note_find_segment_ranges(
    const HirariAudioNoteSegmentRange* ranges, size_t range_count,
    double seconds, int64_t* output_indices);
extern "C" bool hirari_audio_note_validate_segments(
    const HirariAudioNoteSegmentValidation* segments, size_t segment_count,
    uint32_t* ordered_indices);
extern "C" int64_t hirari_audio_note_upsert_segment_slot(
    const HirariAudioNoteSegmentRange* ranges, size_t range_count,
    double start_seconds, double end_seconds);
extern "C" int64_t hirari_audio_note_find_segment_for_edit(
    const HirariAudioNoteSegmentRange* ranges, size_t range_count,
    double start_seconds);
extern "C" bool hirari_audio_note_can_warp_segment(
    const HirariAudioNoteSegmentRange* ranges, size_t range_count,
    size_t target_index, double new_start_seconds, double new_end_seconds);
extern "C" bool hirari_audio_note_rebuild_phase_prefixes(
    const HirariAudioNotePhaseView* segments, size_t segment_count,
    uint32_t* ordered_indices, double* corrections);
extern "C" void hirari_audio_note_curve_block(
    const HirariAudioNoteSegmentRange* ranges, size_t range_count,
    const HirariAudioNoteCurveView* curves, size_t curve_count,
    uint64_t first_region_sample, uint64_t region_length, double sample_rate,
    uint32_t frame_count, double* pitch_output, double* formant_output,
    int64_t* matched_indices_output, int64_t* previous_indices_output);
extern "C" bool hirari_region_prepare_block(
    const HirariRegionBlockConfig* config, uint32_t frames,
    const HirariRegionCompRange* comp_ranges, size_t comp_range_count,
    const HirariRegionRangeEdit* range_edits, size_t range_edit_count,
    const HirariAudioNoteSegmentRange* note_ranges, size_t note_range_count,
    const HirariAudioNoteCurveView* note_curves, size_t note_curve_count,
    const HirariWarpMarker* warp_markers, size_t warp_marker_count,
    const HirariRegionBlockOutput* output);
extern "C" void hirari_region_render_chunks(
    const float* source_left, const float* source_right,
    const HirariRegionBlockConfig* block_template,
    const HirariRegionRenderConfig* render_config,
    const HirariRegionCompRange* comp_ranges, size_t comp_range_count,
    const HirariRegionRangeEdit* range_edits, size_t range_edit_count,
    const HirariAudioNoteSegmentRange* note_ranges, size_t note_range_count,
    const HirariAudioNoteCurveView* note_curves, size_t note_curve_count,
    const HirariWarpMarker* warp_markers, size_t warp_marker_count,
    const float* resample_kernel, const float* wsola_window,
    void* stretch_handle,
    float* destination_left, float* destination_right, uint32_t frames);
extern "C" bool hirari_region_render_block(
    const float* source_left, const float* source_right,
    const HirariRegionBlockConfig* block_config,
    const HirariRegionRenderConfig* render_config,
    const HirariRegionBlockOutput* prepared,
    const HirariWarpMarker* warp_markers, size_t warp_marker_count,
    const HirariAudioNoteSegmentRange* note_ranges, size_t note_range_count,
    const HirariAudioNoteCurveView* note_curves, size_t note_curve_count,
    const float* kernel, const float* window,
    const float* spectral_left, const float* spectral_right,
    float* destination_left, float* destination_right, uint32_t frames);
extern "C" bool hirari_region_needs_spectral_stretch(
    double source_rate, size_t warp_marker_count, float pitch_semitones,
    const HirariAudioNoteCurveView* note_curves, size_t note_curve_count);
extern "C" void hirari_audio_note_curve_build_integral_prefix(
    const HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double pitch_offset_cents, double* prefix_output);
extern "C" size_t hirari_audio_note_curve_normalize_anchors(
    HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double segment_start, double segment_end);
extern "C" bool hirari_audio_note_curve_warp_anchors(
    HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double old_start, double old_end, double new_start, double new_end);
extern "C" size_t hirari_audio_note_curve_upsert_anchor(
    HirariAudioNoteAnchor* anchors, size_t anchor_count, size_t capacity,
    double segment_start, double segment_end,
    double position_seconds, double pitch_cents, double formant_cents);
extern "C" size_t hirari_audio_note_curve_move_anchor(
    HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double segment_start, double segment_end,
    double old_position, double new_position, double value_cents,
    uint8_t edit_formant, double pitch_offset, double formant_offset);
extern "C" bool hirari_audio_note_curve_component_anchor(
    const HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double segment_start, double segment_end, double position_seconds,
    double value_cents, uint8_t edit_formant,
    double pitch_offset, double formant_offset, HirariAudioNoteAnchor* output);
extern "C" void hirari_audio_note_curve_at(
    const HirariAudioNoteAnchor* anchors, size_t anchor_count,
    double seconds, double pitch_offset_cents, double formant_offset_cents,
    double* output_pitch_formant);
extern "C" double hirari_audio_note_curve_integral_at(
    const HirariAudioNoteAnchor* anchors, size_t anchor_count,
    const double* prefix, double start_seconds, double end_seconds,
    double pitch_offset_cents, double seconds);
extern "C" double hirari_wsola_select_grain(
    const float* left, const float* right, uint64_t source_samples,
    uint64_t source_offset, uint64_t source_span, double expected,
    double reference, double overlap_span, double reference_span,
    int64_t center, int64_t last, int64_t search_radius, uint8_t reverse,
    const double* current_offsets, const double* previous_offsets,
    uint32_t point_count);
extern "C" void hirari_wsola_render_frame(
    const float* left, const float* right, uint64_t source_samples,
    uint64_t source_offset, uint64_t source_span, uint8_t reverse,
    double resample_step, const float* kernel, const double* positions,
    const float* weights, uint32_t grain_count, float* output);
extern "C" void hirari_region_wsola_frame(
    const float* source_left, const float* source_right,
    uint64_t source_samples, uint64_t source_offset, uint64_t source_span,
    uint64_t region_length, uint64_t loop_relative, double sample_rate,
    double source_rate, double base_pitch_ratio, double effective_pitch_ratio,
    const HirariWarpMarker* markers, size_t marker_count,
    const HirariAudioNoteSegmentRange* note_ranges,
    const HirariAudioNoteCurveView* note_curves, size_t note_segment_count,
    uint64_t* cache_grain_ids, double* cache_grain_starts, size_t cache_capacity,
    uint32_t sync_group, uint8_t reverse, const float* kernel,
    const float* window, float* output);
extern "C" void* hirari_true_peak_limiter_create(double sample_rate);
extern "C" void hirari_true_peak_limiter_destroy(void* state);
extern "C" void hirari_true_peak_limiter_prepare(void* state, double sample_rate);
extern "C" void hirari_true_peak_limiter_reset(void* state);
extern "C" void hirari_true_peak_limiter_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void hirari_true_peak_limiter_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_true_peak_limiter_get_parameter(
    const void* state, uint32_t id);
extern "C" bool hirari_true_peak_limiter_write_state(
    const void* state, uint8_t* output, size_t output_size);
extern "C" bool hirari_true_peak_limiter_restore_state(
    const void* state, const uint8_t* input, size_t input_size);
extern "C" uint32_t hirari_true_peak_limiter_latency(const void* state);
extern "C" uint32_t hirari_true_peak_limiter_tail(const void* state);
extern "C" void* hirari_auto_pitch_create(double sample_rate, size_t fft_size);
extern "C" void hirari_auto_pitch_destroy(void* state);
extern "C" void hirari_auto_pitch_prepare(void* state, double sample_rate);
extern "C" void hirari_auto_pitch_reset(void* state);
extern "C" void hirari_auto_pitch_process(void* state, float* left, float* right, size_t frames);
extern "C" void hirari_auto_pitch_set_parameter(const void* state, uint32_t id, float value);
extern "C" float hirari_auto_pitch_get_parameter(const void* state, uint32_t id);
extern "C" uint32_t hirari_auto_pitch_latency(const void* state);
extern "C" float hirari_auto_pitch_detected_frequency(const void* state);
extern "C" float hirari_auto_pitch_correction_amount(const void* state);
extern "C" void* hirari_vocal_tuner_create();
extern "C" void hirari_vocal_tuner_destroy(void* state);
extern "C" void hirari_vocal_tuner_prepare(const void* state, double sample_rate);
extern "C" void hirari_vocal_tuner_reset(const void* state);
extern "C" void hirari_vocal_tuner_process(
    const void* state, float* left, float* right, size_t frames);
extern "C" void hirari_vocal_tuner_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_vocal_tuner_get_parameter(const void* state, uint32_t id);
extern "C" float hirari_vocal_tuner_detected_frequency(const void* state);
extern "C" void* hirari_linear_phase_eq_create();
extern "C" void hirari_linear_phase_eq_destroy(void* state);
extern "C" void hirari_linear_phase_eq_set_gains(
    const void* state, float low, float mid, float high);
extern "C" float hirari_linear_phase_eq_get_gain(const void* state, uint32_t band);
extern "C" void hirari_linear_phase_eq_reset(const void* state);
extern "C" void hirari_linear_phase_eq_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_convolution_reverb_create(double sample_rate);
extern "C" void hirari_convolution_reverb_destroy(void* state);
extern "C" void hirari_convolution_reverb_prepare(void* state, double sample_rate);
extern "C" uint32_t hirari_convolution_reverb_tail(const void* state);
extern "C" void hirari_convolution_reverb_reset(void* state);
extern "C" void hirari_convolution_reverb_set_ir(void* state, uint32_t model);
extern "C" bool hirari_convolution_reverb_load_ir(
    void* state, const float* samples, size_t sample_count);
extern "C" void hirari_convolution_reverb_process(
    void* state, float* left, float* right, size_t frames, float mix);
extern "C" void* hirari_native_noise_gate_create();
extern "C" void hirari_native_noise_gate_destroy(void* state);
extern "C" void hirari_native_noise_gate_reset(const void* state);
extern "C" void hirari_native_noise_gate_set_threshold(const void* state, float value);
extern "C" float hirari_native_noise_gate_get_threshold(const void* state);
extern "C" void hirari_native_noise_gate_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_transient_shaper_create();
extern "C" void hirari_native_transient_shaper_destroy(void* state);
extern "C" void hirari_native_transient_shaper_reset(const void* state);
extern "C" void hirari_native_transient_shaper_set_amount(const void* state, float value);
extern "C" float hirari_native_transient_shaper_get_amount(const void* state);
extern "C" void hirari_native_transient_shaper_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_deesser_create();
extern "C" void hirari_native_deesser_destroy(void* state);
extern "C" void hirari_native_deesser_reset(const void* state);
extern "C" void hirari_native_deesser_set_amount(const void* state, float value);
extern "C" float hirari_native_deesser_get_amount(const void* state);
extern "C" void hirari_native_deesser_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_delay_create();
extern "C" void hirari_native_delay_destroy(void* state);
extern "C" void hirari_native_delay_prepare(const void* state, double sample_rate);
extern "C" void hirari_native_delay_reset(const void* state);
extern "C" void hirari_native_delay_set_mix(const void* state, float value);
extern "C" float hirari_native_delay_get_mix(const void* state);
extern "C" void hirari_native_delay_process(
    const void* state, float* const* channels, uint32_t channel_count,
    uint32_t frames, double bpm);
extern "C" void* hirari_native_reverb_create();
extern "C" void hirari_native_reverb_destroy(void* state);
extern "C" void hirari_native_reverb_reset(const void* state);
extern "C" void hirari_native_reverb_set_mix(const void* state, float value);
extern "C" float hirari_native_reverb_get_mix(const void* state);
extern "C" void hirari_native_reverb_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_dynamic_eq_create();
extern "C" void hirari_native_dynamic_eq_destroy(void* state);
extern "C" void hirari_native_dynamic_eq_reset(const void* state);
extern "C" void hirari_native_dynamic_eq_set_amount(const void* state, float value);
extern "C" float hirari_native_dynamic_eq_get_amount(const void* state);
extern "C" void hirari_native_dynamic_eq_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_mid_side_create();
extern "C" void hirari_native_mid_side_destroy(void* state);
extern "C" void hirari_native_mid_side_set(const void* state, float value);
extern "C" float hirari_native_mid_side_get(const void* state);
extern "C" void hirari_native_mid_side_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_native_stereo_width_create();
extern "C" void hirari_native_stereo_width_destroy(void* state);
extern "C" void hirari_native_stereo_width_set(const void* state, float value);
extern "C" float hirari_native_stereo_width_get(const void* state);
extern "C" void hirari_native_stereo_width_process(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void* hirari_pro_limiter_create(double sample_rate);
extern "C" void hirari_pro_limiter_destroy(void* state);
extern "C" void hirari_pro_limiter_prepare(void* state, double sample_rate);
extern "C" void hirari_pro_limiter_reset(void* state);
extern "C" void hirari_pro_limiter_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void hirari_pro_limiter_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_pro_limiter_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_pro_limiter_set_threshold_db(const void* state, float db);
extern "C" float hirari_pro_limiter_threshold_linear(const void* state);
extern "C" bool hirari_pro_limiter_restore_threshold(const void* state, float threshold);
extern "C" uint32_t hirari_pro_limiter_tail(const void* state);
extern "C" void* hirari_tube_saturation_create();
extern "C" void hirari_tube_saturation_destroy(void* state);
extern "C" void hirari_tube_saturation_reset(void* state);
extern "C" void hirari_tube_saturation_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_tube_saturation_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_tube_saturation_set_control(
    const void* state, uint32_t control, float value);
extern "C" void hirari_tube_saturation_process(
    const void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_sub_bass_create(double sample_rate);
extern "C" void hirari_sub_bass_destroy(void* state);
extern "C" void hirari_sub_bass_prepare(void* state, double sample_rate);
extern "C" void hirari_sub_bass_reset(void* state);
extern "C" void hirari_sub_bass_set_parameter(const void* state, uint32_t id, float value);
extern "C" float hirari_sub_bass_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_sub_bass_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_fet_compressor_create(double sample_rate);
extern "C" void hirari_fet_compressor_destroy(void* state);
extern "C" void hirari_fet_compressor_prepare(void* state, double sample_rate);
extern "C" void hirari_fet_compressor_reset(void* state);
extern "C" void hirari_fet_compressor_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_fet_compressor_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_fet_compressor_set_control(
    const void* state, uint32_t control, float value);
extern "C" void hirari_fet_compressor_set_parameters(
    const void* state, float input_db, float output_db, float threshold_db,
    float attack_ms, float release_ms, int32_t ratio);
extern "C" void hirari_fet_compressor_process(
    void* state, float** channels, size_t channel_count, size_t frames);
extern "C" uint32_t hirari_fet_compressor_tail(const void* state);
extern "C" void* hirari_stereo_tremolo_create(double sample_rate);
extern "C" void hirari_stereo_tremolo_destroy(void* state);
extern "C" void hirari_stereo_tremolo_prepare(void* state, double sample_rate);
extern "C" void hirari_stereo_tremolo_reset(void* state);
extern "C" void hirari_stereo_tremolo_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_stereo_tremolo_get_parameter(const void* state, uint32_t id);
extern "C" float hirari_stereo_tremolo_get_note_value(const void* state);
extern "C" void hirari_stereo_tremolo_set_depth(void* state, float value);
extern "C" void hirari_stereo_tremolo_set_note_value(void* state, float value);
extern "C" void hirari_stereo_tremolo_set_width(void* state, float value);
extern "C" void hirari_stereo_tremolo_process(
    void* state, float* left, float* right, uint32_t frames, double bpm);
extern "C" void* hirari_bitcrusher_create();
extern "C" void hirari_bitcrusher_destroy(void* state);
extern "C" void hirari_bitcrusher_reset(void* state);
extern "C" void hirari_bitcrusher_set_bits(void* state, float bits);
extern "C" void hirari_bitcrusher_set_downsample(void* state, float downsample);
extern "C" void hirari_bitcrusher_process(
    void* state, float* left, float* right, uint32_t frames, float mix);
extern "C" void* hirari_stereo_chorus_create(double sample_rate);
extern "C" void hirari_stereo_chorus_destroy(void* state);
extern "C" void hirari_stereo_chorus_prepare(void* state, double sample_rate);
extern "C" void hirari_stereo_chorus_reset(void* state);
extern "C" void hirari_stereo_chorus_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_stereo_chorus_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_stereo_chorus_set_rate(void* state, float rate);
extern "C" void hirari_stereo_chorus_set_mix(void* state, float mix);
extern "C" void hirari_stereo_chorus_process(
    void* state, float* left, float* right, uint32_t frames, float mix);
extern "C" void* hirari_stereo_phaser_create(double sample_rate);
extern "C" void hirari_stereo_phaser_destroy(void* state);
extern "C" void hirari_stereo_phaser_prepare(void* state, double sample_rate);
extern "C" void hirari_stereo_phaser_reset(void* state);
extern "C" void hirari_stereo_phaser_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_stereo_phaser_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_stereo_phaser_set_rate(void* state, float value);
extern "C" void hirari_stereo_phaser_set_feedback(void* state, float value);
extern "C" void hirari_stereo_phaser_process(
    void* state, float* left, float* right, uint32_t frames, float mix);
extern "C" uint32_t hirari_stereo_phaser_tail(const void* state);
extern "C" void* hirari_virtuoso_tape_create(double sample_rate);
extern "C" void hirari_virtuoso_tape_destroy(void* state);
extern "C" void hirari_virtuoso_tape_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_tape_reset(void* state);
extern "C" void hirari_virtuoso_tape_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" float hirari_virtuoso_tape_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_virtuoso_tape_set_control(
    const void* state, uint32_t control, float value);
extern "C" void hirari_virtuoso_tape_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_virtuoso_pultec_create(double sample_rate);
extern "C" void hirari_virtuoso_pultec_destroy(void* state);
extern "C" void hirari_virtuoso_pultec_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_pultec_reset(void* state);
extern "C" void hirari_virtuoso_pultec_set_parameter(
    const void* state, uint32_t id, float value);
extern "C" void hirari_virtuoso_pultec_set_direct(
    const void* state, uint32_t id, float value);
extern "C" float hirari_virtuoso_pultec_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_virtuoso_pultec_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void* hirari_tpdf_dither_create();
extern "C" void hirari_tpdf_dither_destroy(void* state);
extern "C" float hirari_tpdf_dither_next(void* state);
extern "C" void hirari_tpdf_dither_process(void* state, float* samples, size_t frames);
extern "C" void* hirari_noise_shaping_dither_create();
extern "C" void hirari_noise_shaping_dither_destroy(void* state);
extern "C" float hirari_noise_shaping_dither_process_sample(void* state, float sample, int32_t bits);
extern "C" void hirari_noise_shaping_dither_process(
    void* state, float* samples, size_t frames, int32_t bits);
extern "C" void* hirari_master_suite_create(double sample_rate);
extern "C" void hirari_master_suite_destroy(void* handle);
extern "C" void hirari_master_suite_prepare(void* handle, double sample_rate);
extern "C" void hirari_master_suite_reset(void* handle);
extern "C" void hirari_master_suite_process(
    void* handle, float* left, float* right, size_t frames);
extern "C" float hirari_master_suite_gain(const void* handle);
extern "C" void hirari_master_suite_set_auto_gain(
    const void* handle, bool enabled, float target_lufs);
extern "C" void hirari_master_suite_set_mid_side(const void* handle, bool enabled);
extern "C" void hirari_master_suite_set_width(const void* handle, float width);
extern "C" void hirari_master_suite_set_dither(const void* handle, bool enabled);
extern "C" float hirari_master_suite_meter_value(const void* handle, uint32_t field);
extern "C" void hirari_master_suite_meter_analysis_stats(const void* handle, float* output);
extern "C" float hirari_master_suite_meter_spectrum_band(
    const void* handle, uint32_t channel, uint32_t band);
extern "C" bool hirari_master_suite_meter_goniometer(
    const void* handle, float* left, float* right, size_t frames,
    float* correlation, float* balance);
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
extern "C" void* hirari_hws_create(double sample_rate);
extern "C" void hirari_hws_destroy(void* state);
extern "C" void hirari_hws_prepare(void* state, double sample_rate);
extern "C" void hirari_hws_reset(void* state);
extern "C" void hirari_hws_note_on(void* state, double frequency, float velocity);
extern "C" void hirari_hws_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, uint32_t frames);
extern "C" void* hirari_ws_create(double sample_rate);
extern "C" void hirari_ws_destroy(void* state);
extern "C" void hirari_ws_prepare(void* state, double sample_rate);
extern "C" void hirari_ws_reset(void* state);
extern "C" void hirari_ws_note_on(void* state, uint8_t note, uint8_t velocity);
extern "C" void hirari_ws_note_off(void* state, uint8_t note);
extern "C" void hirari_ws_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, uint32_t frames);
extern "C" void* hirari_step_filter_create(double sample_rate);
extern "C" void hirari_step_filter_destroy(void* state);
extern "C" void hirari_step_filter_prepare(const void* state, double sample_rate);
extern "C" void hirari_step_filter_reset(const void* state);
extern "C" void hirari_step_filter_set_step(const void* state, uint32_t step, float value);
extern "C" void hirari_step_filter_set_resonance(const void* state, float value);
extern "C" void hirari_step_filter_process(
    const void* state, float** channels, uint32_t channel_count, uint32_t frames,
    double bpm, double context_sample_rate, uint64_t block_start);
extern "C" float hirari_pitch_detector_estimate(
    const float* buffer, size_t size, double sample_rate);
extern "C" uint8_t hirari_pitch_detector_frequency_to_midi(float frequency);
extern "C" void* hirari_svf_create(double sample_rate);
extern "C" void hirari_svf_destroy(void* state);
extern "C" void hirari_svf_reset(const void* state);
extern "C" void hirari_svf_set_sample_rate(const void* state, double sample_rate);
extern "C" void hirari_svf_set_parameters(const void* state, float frequency, float resonance, int32_t mode);
extern "C" void hirari_svf_process_block(const void* state, float* data, uint32_t frames, uint32_t mode);
extern "C" float hirari_svf_process_sample(const void* state, float input, uint32_t mode);
extern "C" void* hirari_goniometer_create();
extern "C" void hirari_goniometer_destroy(void* state);
extern "C" void hirari_goniometer_process(
    const void* state, const float* left, const float* right, size_t frames);
extern "C" bool hirari_goniometer_snapshot(
    const void* state, float* left, float* right, size_t frames,
    float* correlation, float* balance);
extern "C" void* hirari_dynamic_compressor_create(double sample_rate);
extern "C" void hirari_dynamic_compressor_destroy(void* state);
extern "C" void hirari_dynamic_compressor_prepare(void* state, double sample_rate);
extern "C" void hirari_dynamic_compressor_reset(void* state);
extern "C" void hirari_dynamic_compressor_process(
    void* state, float* left, float* right, size_t frames);
extern "C" void hirari_dynamic_compressor_set_parameter(
    void* state, uint32_t id, float value);
extern "C" float hirari_dynamic_compressor_get_parameter(
    const void* state, uint32_t id);
extern "C" void hirari_dynamic_compressor_set_control(
    void* state, uint32_t control, float value);
extern "C" float hirari_dynamic_compressor_get_control(
    const void* state, uint32_t control);
extern "C" bool hirari_dynamic_compressor_restore(
    void* state, float threshold, float ratio, float makeup,
    float knee, uint32_t lookahead);
extern "C" uint32_t hirari_dynamic_compressor_latency(const void* state);
extern "C" uint32_t hirari_dynamic_compressor_tail(const void* state);
extern "C" uint8_t hirari_save_native_project_bytes(
    const char* path, size_t path_size, const uint8_t* data, size_t data_size);
struct HirariNativeProjectHeader {
    uint32_t version;
    uint32_t sample_rate;
    uint32_t header_size;
    uint32_t track_count;
    double bpm;
    int32_t root_note;
    int32_t scale_type;
};
struct HirariProjectBlobView {
    const uint8_t* data;
    size_t size;
};
struct HirariProjectAutomationPoint {
    double time;
    float value;
    float curve;
};
struct HirariProjectPluginAutomationLaneView {
    uint32_t plugin_index;
    uint32_t parameter_id;
    const HirariProjectAutomationPoint* points;
    size_t point_count;
};
struct HirariProjectTrackView {
    uint32_t id;
    uint32_t track_type;
    float volume;
    float pan;
    float pan3d_x;
    float pan3d_y;
    float pan3d_z;
    uint8_t muted;
    uint8_t solo;
    uint8_t phase_invert;
    uint8_t record_armed;
    uint32_t track_delay_samples;
    const uint8_t* plugin_name;
    size_t plugin_name_size;
    const uint8_t* plugin_data;
    size_t plugin_data_size;
    const HirariProjectBlobView* plugin_states;
    size_t plugin_state_count;
    const HirariProjectBlobView* plugin_gui_states;
    size_t plugin_gui_state_count;
    const uint8_t* plugin_bypass;
    size_t plugin_bypass_count;
    const HirariProjectBlobView* sandboxed_plugin_paths;
    size_t sandboxed_plugin_path_count;
    const HirariProjectBlobView* sandboxed_plugin_states;
    size_t sandboxed_plugin_state_count;
    const HirariProjectAutomationPoint* volume_automation;
    size_t volume_automation_count;
    const HirariProjectAutomationPoint* pan_automation;
    size_t pan_automation_count;
    const HirariProjectAutomationPoint* track_delay_automation;
    size_t track_delay_automation_count;
    const HirariProjectPluginAutomationLaneView* plugin_automation;
    size_t plugin_automation_count;
};
using HirariProjectTrackConsumer = bool (*)(void*, const HirariProjectTrackView*);
extern "C" bool hirari_project_decode_tracks(
    const uint8_t* bytes, size_t byte_count, uint32_t header_size, uint32_t version,
    uint32_t expected_track_count, void* context, HirariProjectTrackConsumer consume_track,
    size_t* consumed_offset, size_t* total_string_bytes);
extern "C" uint8_t* hirari_project_encode_tracks(
    uint32_t version, const HirariProjectTrackView* tracks, size_t track_count,
    size_t* output_size);
extern "C" void hirari_project_free_encoded_tracks(uint8_t* data, size_t size);
struct HirariProjectRangeEditView {
    uint64_t start;
    uint64_t end;
    float gain;
    uint64_t fade_in;
    uint64_t fade_out;
};
struct HirariProjectWarpMarkerView {
    uint64_t source_sample;
    uint64_t timeline_sample;
    uint8_t transient;
};
struct HirariProjectEventStepView {
    uint32_t id;
    const uint8_t* operation;
    size_t operation_size;
    float parameter;
    uint8_t enabled;
};
struct HirariProjectAudioNoteAnchorView {
    double position_seconds;
    double pitch_cents;
    double formant_cents;
};
struct HirariProjectAudioNoteSegmentView {
    double start_seconds;
    double end_seconds;
    double detected_pitch_cents;
    double pitch_offset_cents;
    double formant_offset_cents;
    const HirariProjectAudioNoteAnchorView* anchors;
    size_t anchor_count;
};
struct HirariProjectRegionView {
    uint32_t id;
    uint32_t track_id;
    uint64_t sample_position;
    uint64_t sample_length;
    uint64_t source_length;
    uint32_t source_sample_rate;
    uint64_t source_offset;
    uint64_t base_start;
    uint64_t base_source_offset;
    uint64_t base_length;
    uint8_t muted;
    const uint8_t* file_path;
    size_t file_path_size;
    const uint8_t* name;
    size_t name_size;
    float clip_gain;
    uint64_t fade_in_samples;
    uint64_t fade_out_samples;
    uint8_t reverse;
    double warp_ratio;
    uint8_t pitch_preserve_warp;
    float pitch_semitones;
    uint32_t loop_count;
    uint8_t locked;
    uint32_t sync_group;
    const HirariProjectRangeEditView* range_edits;
    size_t range_edit_count;
    const HirariProjectEventStepView* processing_history;
    size_t processing_history_count;
    const HirariProjectAudioNoteSegmentView* audio_note_segments;
    size_t audio_note_segment_count;
    const HirariProjectWarpMarkerView* warp_markers;
    size_t warp_marker_count;
};
using HirariProjectRegionConsumer = bool (*)(void*, const HirariProjectRegionView*);
extern "C" bool hirari_project_decode_regions(
    const uint8_t* bytes, size_t byte_count, size_t offset, uint32_t version,
    uint32_t sample_rate, const uint32_t* track_ids, size_t track_id_count,
    size_t initial_string_bytes, void* context, HirariProjectRegionConsumer consume_region,
    size_t* consumed_offset);
extern "C" uint8_t* hirari_project_encode_regions(
    uint32_t version, uint32_t sample_rate, const HirariProjectRegionView* regions,
    size_t region_count, size_t* output_size);
extern "C" void hirari_project_free_encoded_regions(uint8_t* data, size_t size);
struct HirariProjectSidechainView {
    uint32_t source_id;
    uint32_t destination_id;
    uint32_t plugin_index;
    uint32_t tap_point;
};
struct HirariProjectRouteView {
    uint32_t source_id;
    uint32_t destination_id;
    float gain;
    uint8_t send;
    uint8_t pre_fader;
};
struct HirariProjectMarkerView {
    uint64_t sample;
    const uint8_t* name;
    size_t name_size;
    uint32_t color;
};
struct HirariProjectArrangerPartView {
    uint64_t start;
    uint64_t length;
    uint32_t repeats;
    const uint8_t* name;
    size_t name_size;
};
struct HirariProjectTempoEventView {
    uint64_t sample;
    double bpm;
    uint8_t ramp;
};
struct HirariProjectTailView {
    const HirariProjectSidechainView* sidechains;
    size_t sidechain_count;
    const HirariProjectRouteView* routes;
    size_t route_count;
    const HirariProjectMarkerView* markers;
    size_t marker_count;
    const HirariProjectArrangerPartView* arranger_parts;
    size_t arranger_part_count;
    const HirariProjectTempoEventView* tempo_events;
    size_t tempo_event_count;
};
using HirariProjectTailConsumer = bool (*)(void*, const HirariProjectTailView*);
extern "C" bool hirari_project_decode_tail(
    const uint8_t* bytes, size_t byte_count, size_t offset, uint32_t version,
    void* context, HirariProjectTailConsumer consume_tail, size_t* consumed_offset);
extern "C" uint8_t* hirari_project_encode_tail(
    uint32_t version, const HirariProjectTailView* tail, size_t* output_size);
extern "C" void hirari_project_free_encoded_tail(uint8_t* data, size_t size);
extern "C" uint8_t* hirari_project_assemble_native_payload(
    uint32_t version, uint32_t sample_rate, double bpm, int32_t root_note,
    int32_t scale_type, const uint8_t* tracks, size_t tracks_size,
    const uint8_t* regions, size_t regions_size, const uint8_t* tail, size_t tail_size,
    size_t* output_size);
extern "C" void hirari_project_free_assembled_payload(uint8_t* data, size_t size);
extern "C" uint8_t* hirari_load_project_payload(
    const char* path, size_t path_size, size_t* output_size,
    HirariNativeProjectHeader* output_header);
extern "C" void hirari_free_loaded_project_bytes(uint8_t* data, size_t size);
extern "C" uint8_t hirari_plugin_fingerprint(const char* path, size_t path_size,
                                               uint64_t* output);

struct HirariPluginCacheRecordView {
    const char* name;
    size_t name_size;
    const char* path;
    size_t path_size;
    uint32_t plugin_type;
    uint32_t subtype;
    uint64_t fingerprint;
};

struct HirariPluginCacheRecordOwned {
    uint8_t* name;
    size_t name_size;
    uint8_t* path;
    size_t path_size;
    uint32_t plugin_type;
    uint32_t subtype;
    uint64_t fingerprint;
};

extern "C" uint8_t hirari_plugin_cache_encode(
    uint32_t magic, uint32_t version, const HirariPluginCacheRecordView* records,
    size_t record_count, uint8_t** output, size_t* output_size);
extern "C" void hirari_plugin_cache_free(uint8_t* bytes, size_t size);
extern "C" uint8_t hirari_plugin_cache_load(
    const char* path, size_t path_size, uint32_t expected_magic,
    uint32_t expected_version, HirariPluginCacheRecordOwned** output,
    size_t* output_count);
extern "C" void hirari_plugin_cache_records_free(
    HirariPluginCacheRecordOwned* records, size_t count);
struct HirariPluginBlacklistRecord {
    uint8_t* path;
    size_t path_size;
    uint32_t reason;
};
extern "C" void* hirari_plugin_blacklist_create(const uint8_t* path, size_t path_size);
extern "C" void hirari_plugin_blacklist_destroy(void* state);
extern "C" bool hirari_plugin_blacklist_is_blocked(
    const void* state, const char* path, size_t path_size,
    uint64_t fingerprint, bool has_fingerprint);
extern "C" uint32_t hirari_plugin_blacklist_reason(
    const void* state, const char* path, size_t path_size);
extern "C" bool hirari_plugin_blacklist_set(
    void* state, const char* path, size_t path_size, uint32_t reason,
    uint64_t fingerprint, bool has_fingerprint);
extern "C" bool hirari_plugin_blacklist_remove(
    void* state, const char* path, size_t path_size);
extern "C" bool hirari_plugin_blacklist_snapshot(
    const void* state, HirariPluginBlacklistRecord** output, size_t* output_count);
extern "C" void hirari_plugin_blacklist_snapshot_free(
    HirariPluginBlacklistRecord* records, size_t count);
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
extern "C" void* hirari_wav_float32_stream_create(
    const char* path, size_t path_size, uint32_t sample_rate, uint16_t channels);
extern "C" uint8_t hirari_wav_float32_stream_write(
    void* stream, const float* const* channels, uint32_t frame_count);
extern "C" uint8_t hirari_wav_float32_stream_finish(void* stream);
extern "C" uint64_t hirari_wav_float32_stream_frames(const void* stream);
extern "C" void hirari_wav_float32_stream_free(void* stream);
extern "C" uint8_t hirari_audio_export_bounce(
    const char* path, size_t path_size, uint32_t sample_rate, uint32_t bit_depth,
    uint16_t channels, uint64_t start_sample, uint64_t end_sample, uint8_t normalize,
    void* context,
    uint8_t (*reset)(void*),
    uint8_t (*render)(void*, uint64_t, uint32_t, float**),
    uint8_t (*cancelled)(void*),
    void (*progress)(void*, float));
extern "C" void* hirari_wav_pcm_stream_create(
    const char* path, size_t path_size, uint64_t frames, uint32_t sample_rate,
    uint16_t channels, uint16_t bit_depth, uint8_t broadcast_wave);
extern "C" uint8_t hirari_wav_pcm_stream_write(
    void* stream, const float* const* channels, uint32_t frame_count);
extern "C" uint8_t hirari_wav_pcm_stream_finish(void* stream);
extern "C" void hirari_wav_pcm_stream_free(void* stream);
extern "C" void* hirari_wav_wave64_stream_create(
    const char* path, size_t path_size, uint64_t frames, uint32_t sample_rate,
    uint16_t channels);
extern "C" uint8_t hirari_wav_wave64_stream_write(
    void* stream, const float* const* channels, uint32_t frame_count);
extern "C" uint8_t hirari_wav_wave64_stream_finish(void* stream);
extern "C" uint64_t hirari_wav_wave64_stream_frames(const void* stream);
extern "C" void hirari_wav_wave64_stream_free(void* stream);

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

struct HirariMpeState;
extern "C" void* hirari_analyzer_8khz_create(double sample_rate);
extern "C" void hirari_analyzer_8khz_destroy(void* state);
extern "C" void hirari_analyzer_8khz_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_analyzer_8khz_analyze(
    void* state, const float* samples, size_t sample_count);
extern "C" float hirari_analyzer_8khz_get_energy(const void* state);
extern "C" HirariMpeState* hirari_mpe_state_create();
extern "C" void hirari_mpe_state_free(HirariMpeState* state);
extern "C" void hirari_mpe_state_reset(HirariMpeState* state);
extern "C" void hirari_mpe_state_set_enabled(HirariMpeState* state, bool enabled);
extern "C" bool hirari_mpe_state_is_enabled(const HirariMpeState* state);
extern "C" void hirari_midi_process_mpe(
    HirariMpeState* state, const void* events, size_t event_count);
extern "C" void hirari_midi_apply_articulations(
    void* events, size_t event_count, const void* maps, size_t map_count);
extern "C" void hirari_mpe_set_articulation_map(
    HirariMpeState* state, const void* maps, size_t map_count);
extern "C" void hirari_midi_apply_articulations_from_state(
    const HirariMpeState* state, void* events, size_t event_count);
extern "C" bool hirari_mpe_sysex_send(
    HirariMpeState* state, uint32_t manufacturer_id,
    const uint8_t* data, size_t size);
extern "C" bool hirari_mpe_sysex_incoming(
    HirariMpeState* state, const uint8_t* data, size_t size);
extern "C" size_t hirari_mpe_sysex_pop(
    HirariMpeState* state, uint32_t* manufacturer_id,
    uint8_t* output, size_t capacity);
extern "C" bool hirari_midi_sysex_is_valid(const uint8_t* data, size_t size);
extern "C" bool hirari_midi_sysex_size_is_valid(size_t size);
extern "C" uint32_t hirari_midi_sysex_manufacturer_id(
    const uint8_t* data, size_t size);
extern "C" void* hirari_comping_legacy_create();
extern "C" void* hirari_comping_legacy_clone(const void* state);
extern "C" void hirari_comping_legacy_destroy(void* state);
extern "C" bool hirari_comping_legacy_add_take(
    void* state, uint32_t take_id, uint64_t start_sample, uint64_t end_sample);
extern "C" uint32_t hirari_comping_legacy_active_take_at(
    const void* state, uint64_t sample);
extern "C" bool hirari_comping_legacy_set_segment(
    void* state, uint32_t take_id, uint64_t start, uint64_t len);
extern "C" void hirari_comping_legacy_clear(void* state);
extern "C" uint64_t hirari_grid_snap_absolute(
    uint64_t ticks, uint32_t resolution, uint32_t numerator, uint32_t denominator);
extern "C" uint64_t hirari_grid_snap_relative(
    uint64_t original_ticks, uint64_t delta_ticks, uint32_t resolution,
    uint32_t numerator, uint32_t denominator);
extern "C" void* hirari_automation_manager_create();
extern "C" void hirari_automation_manager_destroy(void* state);
extern "C" void hirari_automation_manager_prepare(const void* state, double sample_rate);
extern "C" void hirari_automation_manager_process(const void* state, uint32_t num_samples);
extern "C" float hirari_automation_manager_get_value(
    const void* state, uint32_t track_id, uint32_t parameter_id);
extern "C" bool hirari_automation_manager_set_target(
    const void* state, uint32_t track_id, uint32_t parameter_id, float value);
extern "C" float hirari_automation_manager_get_target(
    const void* state, uint32_t track_id, uint32_t parameter_id);
extern "C" void hirari_automation_manager_reset(const void* state);
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
extern "C" double hirari_tempo_samples_to_beats(
    const void* events, size_t event_count, uint64_t samples, double sample_rate);
extern "C" uint64_t hirari_tempo_beats_to_samples(
    const void* events, size_t event_count, double beats, double sample_rate);
extern "C" bool hirari_tempo_replace_events_at_beats(
    void* events, size_t event_count, double sample_rate);
extern "C" size_t hirari_tempo_add_event_at_sample(
    const void* source, size_t event_count, void* output, size_t output_capacity,
    uint64_t sample_pos, double bpm, bool ramp, double sample_rate);
extern "C" size_t hirari_tempo_remove_event_at_sample(
    const void* source, size_t event_count, void* output, size_t output_capacity,
    uint64_t sample_pos, double sample_rate);
extern "C" double hirari_tempo_bpm_at_sample(
    const void* events, size_t event_count, uint64_t sample_pos);
extern "C" size_t hirari_time_signature_add(
    const void* source, size_t signature_count, void* output, size_t output_capacity,
    const void* tempo_events, size_t tempo_event_count, double beat,
    uint8_t numerator, uint8_t denominator, double sample_rate);
extern "C" size_t hirari_time_signature_remove(
    const void* source, size_t signature_count, void* output, size_t output_capacity,
    double beat, double sample_rate);
extern "C" bool hirari_time_signature_find(
    const void* signatures, size_t signature_count, double beat, void* output);
extern "C" void hirari_time_signature_reset(void* output);
extern "C" void* hirari_tempo_map_create();
extern "C" void hirari_tempo_map_destroy(void* state);
extern "C" bool hirari_tempo_map_add_tempo(
    void* state, uint64_t sample_pos, double bpm, double sample_rate, bool ramp);
extern "C" bool hirari_tempo_map_replace_events_at_beats(
    void* state, const void* events, size_t event_count, double sample_rate);
extern "C" bool hirari_tempo_map_remove_tempo(
    void* state, uint64_t sample_pos, double sample_rate);
extern "C" bool hirari_tempo_map_clear(void* state, double sample_rate, double initial_bpm);
extern "C" bool hirari_tempo_map_add_time_signature(
    void* state, double beat, uint8_t numerator, uint8_t denominator, double sample_rate);
extern "C" bool hirari_tempo_map_remove_time_signature(
    void* state, double beat, double sample_rate);
extern "C" bool hirari_tempo_map_clear_time_signatures(void* state);
extern "C" size_t hirari_tempo_map_get_event_count(const void* state);
extern "C" size_t hirari_tempo_map_copy_events(const void* state, void* output, size_t capacity);
extern "C" bool hirari_tempo_map_get_event_at(
    const void* state, uint64_t sample_pos, void* output);
extern "C" size_t hirari_tempo_map_get_signature_count(const void* state);
extern "C" size_t hirari_tempo_map_copy_signatures(
    const void* state, void* output, size_t capacity);
extern "C" bool hirari_tempo_map_get_signature_at(
    const void* state, double beat, void* output);
extern "C" double hirari_tempo_map_samples_to_beats(
    const void* state, uint64_t samples, double sample_rate);
extern "C" uint64_t hirari_tempo_map_beats_to_samples(
    const void* state, double beats, double sample_rate);
extern "C" double hirari_tempo_map_bpm_at_sample(const void* state, uint64_t sample_pos);
extern "C" bool hirari_tempo_map_set_bpm(void* state, double bpm);
extern "C" float hirari_tempo_map_get_current_bpm(const void* state);
extern "C" void hirari_tempo_map_set_current_bpm(void* state, float bpm);
extern "C" bool hirari_compile_staged_routing_graph(
    const uint32_t* node_ids, size_t node_count,
    const uint32_t* edge_sources, const uint32_t* edge_destinations, size_t edge_count,
    uint32_t* execution_order_out, size_t order_capacity,
    uint32_t* stage_depths_out, size_t depth_capacity);
extern "C" void* hirari_routing_graph_create();
extern "C" void hirari_routing_graph_destroy(void* state);
extern "C" bool hirari_routing_graph_add_connection(
    const void* state, uint32_t source, uint32_t destination,
    float gain, bool send, bool pre_fader);
extern "C" void hirari_routing_graph_remove_connection(
    const void* state, uint32_t source, uint32_t destination, bool send);
extern "C" bool hirari_routing_graph_add_dependency(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_graph_remove_dependency(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_graph_remove_dependencies_for_node(
    const void* state, uint32_t node);
extern "C" void hirari_routing_graph_reset(const void* state);
extern "C" void* hirari_routing_graph_remove_connections_for_node(
    const void* state, uint32_t node);
extern "C" void* hirari_routing_graph_snapshot_create(const void* state);
extern "C" size_t hirari_routing_graph_snapshot_count(const void* snapshot);
extern "C" size_t hirari_routing_graph_snapshot_copy(
    const void* snapshot, void* output, size_t capacity);
extern "C" void hirari_routing_graph_snapshot_destroy(void* snapshot);
extern "C" size_t hirari_routing_graph_build_order(
    const void* state, uint32_t* output, size_t output_capacity);
extern "C" void* hirari_routing_topology_create();
extern "C" void hirari_routing_topology_destroy(void* state);
extern "C" void hirari_routing_topology_mark_dirty(void* state);
extern "C" void hirari_routing_topology_reset(void* state);
extern "C" bool hirari_routing_topology_build(void* topology, const void* graph);
extern "C" bool hirari_routing_topology_copy(
    const void* state, uint32_t* output, size_t output_capacity, uint32_t* count_out);
extern "C" bool hirari_routing_topology_resolve_process_order(
    const void* state, const uint32_t* active_ids, size_t active_count,
    uint32_t* output, size_t output_capacity, uint32_t* count_out);
extern "C" uint32_t hirari_routing_topology_node_count(const void* state);
extern "C" bool hirari_routing_runtime_add_connection(
    const void* graph, void* gains, void* topology,
    uint32_t source, uint32_t destination, float gain, bool send, bool pre_fader);
extern "C" void hirari_routing_runtime_remove_connection(
    const void* graph, void* gains, void* topology,
    uint32_t source, uint32_t destination);
extern "C" void hirari_routing_runtime_remove_send(
    const void* graph, void* gains, void* send_pdc, void* topology,
    uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_runtime_add_dependency(
    const void* graph, void* topology, uint32_t source, uint32_t destination);
extern "C" void hirari_routing_runtime_remove_dependency(
    const void* graph, void* topology, uint32_t source, uint32_t destination);
extern "C" void hirari_routing_runtime_remove_dependencies_for_node(
    const void* graph, void* topology, uint32_t node);
extern "C" void hirari_routing_runtime_reset(
    const void* graph, const void* gains, const void* feedback,
    void* send_pdc, void* topology);
extern "C" void* hirari_routing_runtime_remove_connections_for_node(
    const void* graph, void* gains, void* send_pdc, void* topology,
    uint32_t node);
extern "C" void hirari_routing_fanout_stereo(
    const void* gains, uint32_t source,
    const float* left, const float* right,
    float* const* destination_left, float* const* destination_right,
    const uint32_t* destination_ids, uint32_t destination_count, uint32_t frames);
extern "C" void hirari_routing_fanout_planar(
    const void* gains, uint32_t source,
    const float* const* source_channels,
    float* const* const* destination_channels,
    const uint32_t* destination_ids, uint32_t destination_count,
    uint32_t channel_count, uint32_t frames);
struct HirariRoutingTrackDestination {
    uint32_t id;
    uint8_t can_process;
    uint8_t is_bus;
    uint32_t channel_count;
    float* const* work_channels;
    void* bus_context;
};
using HirariBusPreFxAdd = bool (*)(void*, const float*, const float*, uint32_t, float);
extern "C" void hirari_routing_process_track_fanout(
    const void* gains, void* send_pdc, uint32_t source_id,
    const float* const* source_channels, uint32_t source_channel_count,
    const float* source_send_pre_left, const float* source_send_pre_right,
    const float* source_send_post_left, const float* source_send_post_right,
    uint32_t source_track_delay_samples,
    const HirariRoutingTrackDestination* destinations, size_t destination_count,
    float* master_left, float* master_right, float* send_pdc_left,
    float* send_pdc_right, uint32_t frames, uint32_t max_send_frames,
    HirariBusPreFxAdd bus_add);
extern "C" void* hirari_routing_gains_create();
extern "C" void hirari_routing_gains_destroy(void* state);
extern "C" void hirari_routing_gains_reset(const void* state);
extern "C" bool hirari_routing_gains_set_route(
    const void* state, uint32_t source, uint32_t destination, float gain);
extern "C" bool hirari_routing_gains_set_send(
    const void* state, uint32_t source, uint32_t destination, float gain, bool pre_fader);
extern "C" bool hirari_routing_gains_clear_route(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_gains_clear_send(
    const void* state, uint32_t source, uint32_t destination);
extern "C" float hirari_routing_gains_route(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_should_process_node(
    const void* state, uint32_t source, bool offlineTargetActive,
    uint32_t offlineTarget, bool anySolo, bool sourceSolo);
extern "C" float hirari_routing_gains_send(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_gains_has_send(
    const void* state, uint32_t source, uint32_t destination);
extern "C" bool hirari_routing_gains_send_pre_fader(
    const void* state, uint32_t source, uint32_t destination);
extern "C" void* hirari_routing_feedback_create();
extern "C" void hirari_routing_feedback_destroy(void* state);
extern "C" void hirari_routing_feedback_reset(const void* state);
extern "C" bool hirari_routing_feedback_add(
    const void* state, uint32_t source, uint32_t destination, float gain);
extern "C" void hirari_routing_feedback_remove(
    const void* state, uint32_t source, uint32_t destination);
extern "C" size_t hirari_routing_feedback_remove_node(
    const void* state, uint32_t node, void* output_connections, size_t capacity);
extern "C" float hirari_routing_feedback_gain(
    const void* state, uint32_t source, uint32_t destination);
extern "C" size_t hirari_routing_feedback_copy(
    const void* state, void* output_connections, size_t capacity);
extern "C" void hirari_routing_feedback_inject(
    const void* state, uint32_t destination, float* left, float* right, uint32_t frames);
extern "C" void hirari_routing_feedback_capture(
    const void* state, uint32_t source, const float* left, const float* right, uint32_t frames);
extern "C" void* hirari_send_pdc_create();
extern "C" void hirari_send_pdc_destroy(void* handle);
extern "C" bool hirari_send_pdc_set_delay(void* handle, uint32_t samples);
extern "C" bool hirari_send_pdc_process(
    void* handle, const float* input_left, const float* input_right,
    float* output_left, float* output_right, uint32_t frames,
    uint32_t additional_delay, float input_gain);
extern "C" void* hirari_send_pdc_manager_create();
extern "C" void hirari_send_pdc_manager_destroy(void* handle);
extern "C" void hirari_send_pdc_manager_enter_read(void* handle);
extern "C" void hirari_send_pdc_manager_leave_read(void* handle);
extern "C" bool hirari_send_pdc_manager_set_delay(
    void* handle, uint32_t source, uint32_t destination, uint32_t samples);
extern "C" void hirari_send_pdc_manager_retire(
    void* handle, uint32_t source, uint32_t destination);
extern "C" void hirari_send_pdc_manager_retire_all(void* handle);
extern "C" bool hirari_send_pdc_manager_process(
    void* handle, uint32_t source, uint32_t destination,
    const float* input_left, const float* input_right,
    float* output_left, float* output_right, uint32_t frames,
    uint32_t additional_delay, float input_gain);
extern "C" bool hirari_pdc_solve_engine_graph(
    const uint32_t* node_ids, const uint32_t* node_latencies, size_t node_count,
    const uint32_t* edge_sources, const uint32_t* edge_destinations, size_t edge_count,
    uint32_t* edge_delays_out, size_t edge_capacity,
    uint32_t* node_output_latencies_out, uint32_t* node_compensations_out,
    size_t node_output_capacity, uint32_t* global_latency_out);
extern "C" void* hirari_native_pdc_create();
extern "C" void hirari_native_pdc_destroy(void* state);
extern "C" bool hirari_native_pdc_bind_control_thread(const void* state);
extern "C" bool hirari_native_pdc_set_track_destination(
    const void* state, uint32_t track, uint32_t destination);
extern "C" bool hirari_native_pdc_set_bus_destination(
    const void* state, uint32_t bus, uint32_t destination);
extern "C" bool hirari_native_pdc_set_send_route(
    const void* state, uint32_t source, uint32_t destination, bool enabled);
extern "C" bool hirari_native_pdc_set_track_latency(
    const void* state, uint32_t track, uint32_t samples);
extern "C" bool hirari_native_pdc_set_bus_latency(
    const void* state, uint32_t bus, uint32_t samples);
extern "C" bool hirari_native_pdc_set_low_latency_mode(const void* state, bool active);
extern "C" bool hirari_native_pdc_clear_configuration(const void* state);
extern "C" bool hirari_native_pdc_mark_dirty(const void* state);
extern "C" void hirari_native_pdc_reset_for_project(const void* state);
extern "C" void hirari_native_pdc_recalculate(const void* state);
extern "C" bool hirari_native_pdc_audit(const void* state);
extern "C" uint32_t hirari_native_pdc_get_track_offset(const void* state, uint32_t track);
extern "C" uint32_t hirari_native_pdc_get_bus_offset(const void* state, uint32_t bus);
extern "C" uint32_t hirari_native_pdc_get_send_offset(
    const void* state, uint32_t source, uint32_t destination);
extern "C" uint32_t hirari_native_pdc_get_global_latency(const void* state);
extern "C" bool hirari_native_pdc_has_cycle(const void* state);
extern "C" bool hirari_native_pdc_low_latency_mode(const void* state);
extern "C" uint64_t hirari_native_pdc_configuration_generation(const void* state);
extern "C" void* hirari_mastering_processor_create();
extern "C" void hirari_mastering_processor_free(void* processor);
extern "C" bool hirari_mastering_processor_process(
    void* processor, float* const* channels, size_t channel_count,
    size_t frame_count, const float* band_gains, float* momentary_out);
extern "C" bool hirari_mastering_match_profile(
    const float* current, const float* target, float* gains_out, size_t count);
extern "C" bool hirari_mastering_analyze_profile(
    const float* const* channels, size_t channel_count, size_t frame_count,
    float* bins_out);
struct HirariMasteringByteSlice {
    const uint8_t* data;
    size_t size;
};
extern "C" bool hirari_mastering_export_ddp(
    const uint8_t* output_dir, size_t output_dir_size,
    const uint8_t* title, size_t title_size,
    const uint8_t* upc, size_t upc_size,
    const HirariMasteringByteSlice* isrc_codes, size_t isrc_count);
extern "C" void* hirari_spectrum_analyzer_create();
extern "C" void hirari_spectrum_analyzer_destroy(void* state);
extern "C" void hirari_spectrum_analyzer_process(
    void* state, const float* samples, size_t frames, double sample_rate);
extern "C" float hirari_spectrum_analyzer_get_band(const void* state, uint32_t band);
extern "C" void* hirari_legacy_k_weighting_create(double sample_rate);
extern "C" void hirari_legacy_k_weighting_destroy(void* state);
extern "C" void hirari_legacy_k_weighting_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_legacy_k_weighting_reset(void* state);
extern "C" void hirari_legacy_k_weighting_process(
    void* state, float left, float right, float* out_left, float* out_right);
extern "C" void* hirari_analysis_engine_create(double sample_rate);
extern "C" void hirari_analysis_engine_destroy(void* state);
extern "C" void hirari_analysis_engine_update(
    void* state, const float* left, const float* right, uint32_t frames, double sample_rate);
extern "C" void hirari_analysis_engine_get_stats(const void* state, float* output);
extern "C" float hirari_analysis_engine_get_band(const void* state, uint32_t channel, uint32_t band);
extern "C" void* hirari_fft_plan_create(size_t size);
extern "C" void hirari_fft_plan_destroy(void* state);
extern "C" bool hirari_fft_plan_valid(const void* state);
extern "C" void hirari_fft_forward(const void* state, float* real, float* imag);
extern "C" void hirari_fft_inverse(const void* state, float* real, float* imag);

extern "C" size_t hirari_masking_analysis(
    const float* target_mono,
    const float* other_mono,
    const uint32_t* other_track_ids,
    size_t other_count,
    size_t frame_count,
    uint32_t* output_bins,
    float* output_intensities,
    uint32_t* output_track_ids,
    size_t output_capacity);
extern "C" bool hirari_vocal_remove_stereo(float* left, float* right, size_t frames);
extern "C" float hirari_stereo_correlation(const float* left, const float* right, size_t frames);
extern "C" bool hirari_spectral_profile_analyze(
    const float* input, uint32_t size, float* magnitude_output);
extern "C" void* hirari_spectral_matcher_create();
extern "C" void hirari_spectral_matcher_destroy(void* state);
extern "C" void hirari_spectral_matcher_set_reference(
    void* state, const float* values, size_t length);
extern "C" void hirari_spectral_matcher_update_average(
    void* state, const float* values, size_t length);
extern "C" bool hirari_spectral_matcher_calculate(
    const void* state, float* output, size_t capacity);
extern "C" void hirari_spectral_matcher_set_pink_noise_reference(void* state);
extern "C" void* hirari_loudness_meter_create(double sample_rate);
extern "C" void hirari_loudness_meter_destroy(void* state);
extern "C" void hirari_loudness_meter_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_loudness_meter_reset(void* state);
extern "C" void hirari_loudness_meter_process(
    void* state, const float* left, const float* right, uint32_t frames);
extern "C" float hirari_loudness_meter_integrated_lufs(const void* state);
struct HirariLoudnessMetrics {
    float momentary_lufs;
    float short_term_lufs;
    float true_peak_db_l;
    float true_peak_db_r;
    float true_peak_db;
};
extern "C" void* hirari_loudness_analyzer_create(double sample_rate);
extern "C" void hirari_loudness_analyzer_destroy(void* state);
extern "C" void hirari_loudness_analyzer_prepare(void* state, double sample_rate);
extern "C" HirariLoudnessMetrics hirari_loudness_analyzer_process(
    void* state, const float* left, const float* right, size_t frames);
extern "C" HirariLoudnessMetrics hirari_loudness_analyzer_get_metrics(const void* state);
extern "C" void* hirari_transient_detector_create(double sample_rate, float lookahead_ms);
extern "C" void hirari_transient_detector_destroy(void* state);
extern "C" void hirari_transient_detector_analyze(
    void* state, const float* data, size_t length, float threshold);
extern "C" size_t hirari_transient_detector_result_count(const void* state);
extern "C" bool hirari_transient_detector_get_result(
    const void* state, size_t index, uint64_t* sample_index, float* strength);
extern "C" void* hirari_audio_pitch_analyze(
    const float* samples, size_t count, double sample_rate,
    double min_hz, double max_hz, size_t window, size_t hop,
    double threshold, double note_change_threshold_cents,
    size_t note_change_confirmation_frames);
extern "C" size_t hirari_audio_pitch_select_and_decimate(
    const float* const* channels, size_t channel_count,
    size_t source_offset, size_t source_span, size_t stride,
    uint8_t reverse, float* output, size_t output_capacity);
extern "C" bool hirari_audio_pitch_map_segment_to_timeline(
    double* start_seconds, double* end_seconds,
    HirariAudioNoteAnchor* anchors, size_t* anchor_count,
    double source_sample_rate, double timeline_sample_rate,
    uint64_t source_span, uint64_t timeline_length,
    double source_frames_per_timeline_frame,
    const HirariWarpMarker* warp_markers, size_t warp_marker_count);
extern "C" void hirari_audio_pitch_result_destroy(void* state);
extern "C" size_t hirari_audio_pitch_segment_count(const void* state);
extern "C" bool hirari_audio_pitch_get_segment(
    const void* state, size_t index, double* start_seconds,
    double* end_seconds, double* detected_pitch_cents);
extern "C" size_t hirari_audio_pitch_anchor_count(const void* state, size_t segment_index);
extern "C" bool hirari_audio_pitch_get_anchor(
    const void* state, size_t segment_index, size_t anchor_index,
    double* position_seconds, double* pitch_cents, double* formant_cents);
extern "C" size_t hirari_audio_resampler_output_len(
    size_t source_len, double source_rate, double target_rate);
extern "C" bool hirari_audio_resampler_process(
    const float* source, size_t source_len, double source_rate, double target_rate,
    float* output, size_t output_capacity);
struct HirariTempoAnalysisResult {
    float bpm;
    float confidence;
};
extern "C" HirariTempoAnalysisResult hirari_tempo_analyzer_detect_bpm(
    const float* data, uint64_t length, double sample_rate);
extern "C" void* hirari_spectral_editor_create(uint32_t fft_size);
extern "C" void hirari_spectral_editor_destroy(void* state);
extern "C" void hirari_spectral_editor_process(void* state, const float* input, float* output, uint32_t len);
extern "C" void hirari_spectral_editor_process_in_place(void* state, float* samples, uint32_t len);
extern "C" void hirari_spectral_editor_flush(void* state, float* output, uint32_t len);
extern "C" uint32_t hirari_spectral_editor_latency(const void* state);
extern "C" uint32_t hirari_spectral_editor_tail(const void* state);
extern "C" void hirari_spectral_editor_set_learn(void* state, bool active);
extern "C" void hirari_spectral_editor_clear_profile(void* state);
extern "C" bool hirari_spectral_editor_profile_ready(const void* state);
extern "C" void hirari_spectral_editor_set_active(void* state, bool active);
extern "C" void hirari_spectral_editor_set_threshold(void* state, float threshold);
extern "C" void hirari_spectral_editor_set_harmonics(const void* state, float fundamental, float sample_rate, float bandwidth);
extern "C" void hirari_spectral_editor_reset(void* state);
extern "C" void* hirari_spectral_restoration_create(double sample_rate);
extern "C" void hirari_spectral_restoration_destroy(void* state);
extern "C" double hirari_spectral_restoration_sample_rate(const void* state);
extern "C" void hirari_spectral_restoration_prepare(void* state, double sample_rate);
extern "C" void hirari_spectral_restoration_reset(void* state);
extern "C" uint32_t hirari_spectral_restoration_latency(const void* state);
extern "C" uint32_t hirari_spectral_restoration_tail(const void* state);
extern "C" void hirari_spectral_restoration_set_parameter(const void* state, uint32_t id, float value);
extern "C" float hirari_spectral_restoration_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_spectral_restoration_process(void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void hirari_spectral_restoration_flush(void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" void hirari_spectral_restoration_set_learn(const void* state, bool active);
extern "C" void hirari_spectral_restoration_clear_profile(const void* state);
extern "C" bool hirari_spectral_restoration_profile_ready(const void* state);
extern "C" void hirari_spectral_restoration_set_harmonics(const void* state, float fundamental, float bandwidth);
extern "C" bool hirari_spectral_restoration_write_state(
    const void* state, bool bypassed, uint32_t sidechain_bus, uint8_t* output, size_t output_size);
extern "C" bool hirari_spectral_restoration_restore_state(
    void* state, const uint8_t* input, size_t input_size, bool* bypassed, uint32_t* sidechain_bus);
extern "C" bool hirari_spectral_processor_apply_gain(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float f0, float t1, float f1, float gain);
extern "C" bool hirari_spectral_processor_apply_gain_regions(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    const float* regions, uint32_t region_count, float gain);
extern "C" bool hirari_spectral_processor_apply_mask(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    const float* regions, const float* gains, uint32_t region_count);
extern "C" bool hirari_spectral_processor_remove_hum(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float fundamental, uint32_t harmonics, float bandwidth,
    float t0, float f0, float t1, float f1);
extern "C" bool hirari_spectral_processor_interpolate(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float f0, float t1, float f1, float blend);
extern "C" bool hirari_spectral_processor_reduce_noise(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float f0, float t1, float f1, float amount, float profile_seconds);
extern "C" uint32_t hirari_spectral_processor_remove_clicks(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float t1, float threshold, uint32_t radius, bool selected);
extern "C" uint32_t hirari_spectral_processor_repair_clipped(
    float* const* channels, uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float t1, float ceiling, bool selected);
extern "C" void* hirari_spectral_history_create();
extern "C" void hirari_spectral_history_destroy(void* state);
extern "C" bool hirari_spectral_history_capture(
    void* state, uintptr_t owner, uint32_t channels, uint32_t frames,
    const float* const* data, const uint8_t* label, size_t label_len);
extern "C" void* hirari_spectral_history_snapshot_create(
    uintptr_t owner, uint32_t channels, uint32_t frames,
    const float* const* data, const uint8_t* label, size_t label_len);
extern "C" void hirari_spectral_history_snapshot_destroy(void* snapshot);
extern "C" void hirari_spectral_history_token_destroy(void* token);
extern "C" void* hirari_spectral_history_peek(const void* state, uintptr_t owner, bool redo);
extern "C" size_t hirari_spectral_history_label(
    const void* state, uintptr_t owner, bool redo, uint8_t* output, size_t capacity);
extern "C" uint32_t hirari_spectral_history_snapshot_channels(const void* state, const void* token);
extern "C" uint32_t hirari_spectral_history_snapshot_frames(const void* state, const void* token);
extern "C" size_t hirari_spectral_history_snapshot_label(
    const void* state, const void* token, uint8_t* output, size_t capacity);
extern "C" bool hirari_spectral_history_snapshot_restore(
    const void* state, const void* token, float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" bool hirari_spectral_history_finish_restore(
    void* state, uintptr_t owner, bool redo, void* target, void* current);
extern "C" bool hirari_spectral_history_contains(const void* state, uintptr_t owner, bool redo);
extern "C" size_t hirari_spectral_history_depth(const void* state, uintptr_t owner, bool redo);
extern "C" size_t hirari_spectral_history_bytes(const void* state);
extern "C" void hirari_spectral_history_clear(void* state, uintptr_t owner);
using HirariSamplerBufferReader = bool (*)(void*, const float**, const float**, uint32_t*);
extern "C" void* hirari_sampler_create(double sample_rate, HirariSamplerBufferReader reader);
extern "C" void hirari_sampler_destroy(void* state);
extern "C" void hirari_sampler_note_on_buffer(void* state, uint8_t note, uint8_t velocity,
    void* buffer, uint8_t root, double source_rate);
extern "C" void hirari_sampler_add_zone(void* state, const float* data, uint64_t length,
    double source_rate, uint8_t root, uint8_t low_key, uint8_t high_key,
    uint8_t low_velocity, uint8_t high_velocity, uint32_t loop_start,
    uint32_t loop_end, bool loop_enabled);
extern "C" void hirari_sampler_note_on_zone(void* state, uint8_t note, uint8_t velocity, uint8_t channel);
extern "C" void hirari_sampler_note_off(void* state, uint8_t note, uint8_t channel);
extern "C" bool hirari_sampler_any_sustain(const void* state);
extern "C" void hirari_sampler_set_sustain(void* state, uint8_t channel, bool held);
extern "C" void hirari_sampler_pitch_bend(void* state, uint8_t channel, float bend);
extern "C" void hirari_sampler_channel_expression(void* state, uint8_t channel, uint8_t controller, uint8_t value);
extern "C" void hirari_sampler_channel_pressure(void* state, uint8_t channel, uint8_t value);
extern "C" void hirari_sampler_voice_pressure(void* state, uint8_t channel, uint8_t note, uint8_t value);
extern "C" void hirari_sampler_process(void* state, float* left, float* right, uint32_t frames);
extern "C" void hirari_sampler_process_midi_events(void* state, const void* events,
    size_t event_count, float* left, float* right, uint32_t frames);
extern "C" void* hirari_builtin_midi_instrument_create();
extern "C" void hirari_builtin_midi_instrument_destroy(void* state);
extern "C" void hirari_builtin_midi_instrument_process(
    void* state, const void* events, size_t event_count,
    float* left, float* right, uint32_t frames, double sample_rate);
extern "C" float hirari_realtime_automation_evaluate(
    const void* points, size_t count, double time, size_t* last_index_hint);
extern "C" void hirari_realtime_automation_apply_stereo_block(
    const void* gain_points, size_t gain_count, size_t* gain_hint,
    const void* pan_points, size_t pan_count, size_t* pan_hint,
    double start_time, float* left, float* right,
    float* dry_left, float* dry_right, uint32_t frames);
extern "C" void hirari_vca_add_group(uint32_t group_id, float gain);
extern "C" void hirari_vca_resolve_hierarchy();
extern "C" bool hirari_vca_assign_track(uint32_t track_id, uint32_t group_id);
extern "C" float hirari_vca_get_cumulative_gain(uint32_t track_id);
extern "C" void hirari_vca_clear();
extern "C" size_t hirari_vca_snapshot_json(uint8_t* output, size_t capacity);
extern "C" void hirari_vca_apply_track_gain(
    uint32_t track_id, float* left, float* right, uint32_t frames);
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
extern "C" size_t hirari_track_midi_expression_map_copy(
    const void* state, uint8_t map_kind, uint8_t* output, size_t capacity);
extern "C" bool hirari_track_midi_articulation_begin(const void* state, uint8_t articulation_id);
extern "C" bool hirari_track_midi_articulation_set_output(
    const void* state, uint8_t articulation_id, uint8_t slot, uint8_t kind,
    uint8_t channel, uint8_t a, uint8_t b, uint8_t c);
extern "C" bool hirari_track_midi_articulation_finish(
    const void* state, uint8_t articulation_id, uint8_t count);
extern "C" size_t hirari_track_midi_articulation_pack(
    const void* state, uint32_t* output, size_t capacity);
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
struct HirariAutomationEvent {
    uint32_t track_id;
    uint32_t param_id;
    uint64_t pos;
    float val;
};
struct HirariTrackAutomationCapturePoint {
    double time;
    float volume;
    float pan;
};
extern "C" void* hirari_automation_recorder_create();
extern "C" void hirari_automation_recorder_destroy(void* state);
extern "C" void hirari_automation_recorder_set_mode(const void* state, uint32_t mode);
extern "C" uint32_t hirari_automation_recorder_get_mode(const void* state);
extern "C" void hirari_automation_recorder_set_punch_range(
    const void* state, uint64_t start, uint64_t end);
extern "C" void hirari_automation_recorder_record_value(
    const void* state, uint32_t track_id, uint32_t param_id, float value, uint64_t timestamp);
extern "C" void hirari_automation_recorder_flush(const void* state);
extern "C" size_t hirari_automation_recorder_snapshot(
    const void* state, HirariAutomationEvent* output, size_t capacity);
extern "C" void* hirari_track_automation_capture_create();
extern "C" void hirari_track_automation_capture_destroy(void* state);
extern "C" bool hirari_track_automation_capture_record(
    const void* state, const void* recorder, uint32_t track_id,
    uint64_t sample_position, float volume, float pan);
extern "C" size_t hirari_track_automation_capture_flush(
    const void* state, HirariTrackAutomationCapturePoint* output, size_t capacity);
extern "C" float hirari_track_eq_gain(float boost_db, float cut_db);
extern "C" void hirari_track_eq_process(
    float* left, float* right, uint32_t frames,
    float low_gain, float high_gain, float* state_l, float* state_r);
extern "C" void* hirari_track_pdc_delay_create();
extern "C" void hirari_track_pdc_delay_destroy(void* state);
extern "C" void hirari_track_pdc_delay_reset(void* state, uint32_t requested);
extern "C" void hirari_track_pdc_delay_process(
    void* state, float* left, float* right,
    float* pre_left, float* pre_right, uint32_t frames, uint32_t requested);
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
extern "C" void hirari_effect_chain_process_block(
    void* user_data, uint32_t node_count,
    float* const* channels, uint32_t channel_count,
    float* const* parallel_channels, uint32_t parallel_channel_count,
    uint32_t parallel_capacity, uint32_t frames,
    HirariEffectNodeInfo node_info, HirariEffectNodeProcess process_node);
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
extern "C" size_t hirari_plugin_automation_render_block(
    const HirariPluginAutomationLaneView* lanes, size_t lane_count,
    uint64_t playhead, uint32_t frames,
    void* output_events, size_t output_capacity);
struct HirariPolySamplerZoneView {
    uint32_t min_note;
    uint32_t max_note;
    float min_velocity;
    float max_velocity;
    uint32_t root_note;
    uint32_t loop_start;
    uint32_t loop_end;
    bool loop_enabled;
    double source_sample_rate;
    const float* left;
    size_t left_length;
    const float* right;
    size_t right_length;
};
static_assert(sizeof(HirariPolySamplerZoneView) == 72,
    "HirariPolySamplerZoneView must match Rust's repr(C) layout");
extern "C" void* hirari_poly_sampler_create(double sample_rate);
extern "C" void hirari_poly_sampler_destroy(void* state);
extern "C" void hirari_poly_sampler_set_zones(
    void* state, const HirariPolySamplerZoneView* zones, size_t count);
extern "C" void hirari_poly_sampler_note_on(void* state, uint32_t note, float velocity);
extern "C" void hirari_poly_sampler_note_off(void* state, uint32_t note);
extern "C" bool hirari_poly_sampler_set_loop(void* state, uint64_t start, uint64_t end, bool enabled);
extern "C" bool hirari_poly_sampler_is_looping(const void* state);
extern "C" uint64_t hirari_poly_sampler_loop_start(const void* state);
extern "C" uint64_t hirari_poly_sampler_loop_end(const void* state);
extern "C" void hirari_poly_sampler_process(
    void* state, float* left, float* right, size_t frames, bool clear);
extern "C" void hirari_sampler_prepare(void* state, double sample_rate);
extern "C" void hirari_sampler_reset(void* state);
extern "C" size_t hirari_sampler_streaming_count(const void* state);
extern "C" bool hirari_sampler_streaming_at(const void* state, size_t index);
extern "C" void* hirari_neural_synth_create();
extern "C" void hirari_neural_synth_destroy(void* state);
extern "C" void hirari_neural_synth_note_on(void* state, uint8_t note, uint8_t velocity);
extern "C" void hirari_neural_synth_note_off(void* state, uint8_t note);
extern "C" void hirari_neural_synth_process(void* state, float* left, float* right,
    uint32_t frames, double sample_rate, float morph);
extern "C" bool hirari_audio_quantize(
    const float* input, float* output, uint64_t length, float bpm,
    double sample_rate, float strength, float swing);
extern "C" bool hirari_audio_quantize_group(
    const float* const* inputs, float* const* outputs, uint32_t channels,
    uint64_t length, float bpm, double sample_rate, float strength, float swing);
extern "C" bool hirari_midi_quantize(
    void* notes, size_t count, uint32_t resolution, float swing,
    float strength, bool with_length);
extern "C" bool hirari_midi_region_quantize(
    void* notes, size_t count, double grid, double strength,
    double selection_start, double selection_end);
extern "C" void hirari_midi_region_transpose(
    void* notes, size_t count, int32_t semitones,
    double selection_start, double selection_end);
extern "C" size_t hirari_midi_region_remove_notes_at(
    void* notes, size_t count, double beat, int32_t pitch, double tolerance);
extern "C" void hirari_midi_region_set_muted_at(
    void* notes, size_t count, double beat, int32_t pitch, bool muted);
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
extern "C" bool hirari_midi_transform_notes(
    void* notes, size_t count,
    int32_t min_pitch, int32_t max_pitch,
    int32_t min_velocity, int32_t max_velocity,
    uint64_t min_length, uint64_t max_length,
    int32_t pitch_offset, float velocity_scale, int32_t humanize_ticks);
extern "C" bool hirari_midi_apply_scale_quantize(
    void* notes, size_t count, uint8_t root,
    const int32_t* scale, size_t scale_count);
extern "C" void* hirari_scale_quantizer_create();
extern "C" void hirari_scale_quantizer_destroy(void* state);
extern "C" void hirari_scale_quantizer_set(void* state, int32_t root, uint32_t scale);
extern "C" int32_t hirari_scale_quantizer_note(const void* state, int32_t note);
extern "C" bool hirari_scale_quantizer_add_chord(
    void* state, uint64_t tick, int32_t root,
    const int32_t* intervals, size_t interval_count,
    const uint8_t* name, size_t name_length);
extern "C" uint8_t hirari_scale_quantizer_chord_at(
    const void* state, uint64_t tick, int32_t* root_out,
    int32_t* intervals_out, size_t intervals_capacity, size_t* intervals_length_out,
    uint8_t* name_out, size_t name_capacity, size_t* name_length_out);
extern "C" void* hirari_midi_sequencer_create();
extern "C" void hirari_midi_sequencer_destroy(void* state);
extern "C" bool hirari_midi_sequencer_record(
    void* state, uint32_t region_id, uint8_t pitch, uint8_t velocity,
    uint64_t start_tick, uint64_t length);
extern "C" void hirari_midi_sequencer_clear(void* state);
extern "C" void* hirari_midi_sequencer_chase_snapshot(
    const void* state, uint64_t current_tick);
extern "C" void hirari_midi_sequencer_snapshot_destroy(void* snapshot);
extern "C" size_t hirari_midi_sequencer_snapshot_count(const void* snapshot);
extern "C" bool hirari_midi_sequencer_snapshot_copy(
    const void* snapshot, void* output, size_t capacity);
extern "C" void* hirari_retrospective_midi_create();
extern "C" void hirari_retrospective_midi_destroy(void* state);
extern "C" void hirari_retrospective_midi_record(
    void* state, uint32_t track_id, uint8_t status,
    uint8_t data1, uint8_t data2, uint64_t tick);
extern "C" void* hirari_retrospective_midi_flush_snapshot(
    const void* state, uint64_t current_tick, uint64_t lookback_ticks);
extern "C" void hirari_retrospective_midi_snapshot_destroy(void* snapshot);
extern "C" size_t hirari_retrospective_midi_snapshot_count(const void* snapshot);
extern "C" bool hirari_retrospective_midi_snapshot_copy(
    const void* snapshot, void* output, size_t capacity);
extern "C" void* hirari_integer_delay_create(uint32_t max_delay_samples);
extern "C" void hirari_integer_delay_destroy(void* state);
extern "C" float hirari_integer_delay_process(void* state, float sample, uint32_t delay_samples);
extern "C" void hirari_integer_delay_push(void* state, float sample);
extern "C" float hirari_integer_delay_read(const void* state, uint32_t delay_samples);
extern "C" void hirari_integer_delay_reset(void* state);
extern "C" void* hirari_all_pass_create(size_t delay_samples, float feedback);
extern "C" void hirari_all_pass_destroy(void* state);
extern "C" void hirari_all_pass_reset(void* state);
extern "C" float hirari_all_pass_process(void* state, float input);
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
extern "C" bool hirari_midi_extended_ring_pop(void* state, void* destination_message);
extern "C" size_t hirari_midi_extended_ring_size(const void* state);
extern "C" void* hirari_analog_saturator_create(double sample_rate);
extern "C" void hirari_analog_saturator_destroy(void* state);
extern "C" void hirari_analog_saturator_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_analog_saturator_reset(void* state);
extern "C" void hirari_analog_saturator_process(
    void* state, float* left, float* right, uint32_t frames,
    float drive, float warmth, uint32_t model);
extern "C" void* hirari_virtuoso_space_create(double sample_rate);
extern "C" void hirari_virtuoso_space_destroy(void* state);
extern "C" void hirari_virtuoso_space_prepare(void* state, double sample_rate);
extern "C" void hirari_virtuoso_space_set_parameter(void* state, uint32_t id, float value);
extern "C" float hirari_virtuoso_space_get_parameter(const void* state, uint32_t id);
extern "C" void hirari_virtuoso_space_reset(void* state);
extern "C" uint32_t hirari_virtuoso_space_tail_samples(const void* state);
extern "C" void hirari_virtuoso_space_process(
    void* state, float* left, float* right, uint32_t frames);
extern "C" size_t hirari_virtuoso_space_write_state(
    const void* state, uint8_t* output, size_t capacity, bool bypassed,
    uint32_t sidechain_bus_id);
extern "C" bool hirari_virtuoso_space_set_state(
    void* state, const uint8_t* input, size_t length);
extern "C" void* hirari_realtime_memory_pool_create();
extern "C" void hirari_realtime_memory_pool_destroy(void* state);
extern "C" void hirari_realtime_memory_pool_reset(const void* state, uint32_t frame);
extern "C" void* hirari_realtime_memory_pool_allocate(const void* state, size_t size_bytes);
extern "C" void* hirari_status_queue_init(void* storage, size_t bytes);
extern "C" void hirari_status_queue_destroy(void* state);
extern "C" bool hirari_status_queue_push(
    const void* state, uint32_t severity, const uint8_t* text, size_t text_size);
extern "C" bool hirari_status_queue_pop(const void* state, void* output_message);
extern "C" uint64_t hirari_status_queue_dropped(const void* state);
extern "C" uint64_t hirari_status_queue_take_dropped(const void* state);
using HirariVoiceTriggerCallback = void (*)(void*, uint8_t, uint8_t);
using HirariVoiceReleaseCallback = void (*)(void*, uint8_t);
using HirariVoiceRenderCallback = void (*)(void*, float*, float*, size_t);
extern "C" void hirari_voice_manager_render_midi(
    void* midi_state, void* user_data, float* left, float* right, size_t frames,
    HirariVoiceTriggerCallback trigger, HirariVoiceReleaseCallback release,
    HirariVoiceRenderCallback render_range);
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
extern "C" void* hirari_audio_graph_dry_state_create();
extern "C" void hirari_audio_graph_dry_state_destroy(void* state);
extern "C" bool hirari_audio_graph_process_dry(
    void* state, const float* const* sources, float* const* destinations,
    uint32_t channels, uint32_t frames, uint32_t latency);
extern "C" uint64_t hirari_audio_graph_sanitize(
    float* const* channels, uint32_t channel_count, uint32_t frames);
extern "C" bool hirari_audio_graph_blend(
    float* const* wet_channels, const float* const* dry_channels,
    uint32_t channel_count, uint32_t frames, float mix);
extern "C" void* hirari_audio_graph_runtime_create();
extern "C" void hirari_audio_graph_runtime_destroy(void* state);
extern "C" void hirari_audio_graph_runtime_reset_nodes(void* state, size_t count);
extern "C" bool hirari_audio_graph_runtime_append_node(void* state);
extern "C" bool hirari_audio_graph_runtime_remove_node(void* state, size_t index);
extern "C" void hirari_audio_graph_runtime_reset_node_fault(const void* state, size_t index);
extern "C" uint32_t hirari_audio_graph_runtime_faulted_node_count(const void* state);
extern "C" uint64_t hirari_audio_graph_runtime_metric(const void* state, uint32_t metric);
extern "C" void hirari_audio_graph_runtime_note_rejected(const void* state);
using HirariAudioGraphNodeMetrics = bool (*)(void*, uint32_t, uint32_t*, float*);
extern "C" uint32_t hirari_audio_graph_total_latency(
    void* user_data, uint32_t node_count, HirariAudioGraphNodeMetrics read_node);
extern "C" bool hirari_audio_graph_validate_nodes(
    void* user_data, uint32_t node_count, bool prepared,
    HirariAudioGraphNodeMetrics read_node);
using HirariAudioGraphNodeInfo = bool (*)(
    void*, uint32_t, bool*, float*, uint32_t*, void**);
using HirariAudioGraphProcessNode = uint8_t (*)(void*, uint32_t);
extern "C" bool hirari_audio_graph_process_nodes(
    const void* runtime_state, void* user_data,
    float* const* channels, float* const* dry_channels,
    uint32_t channel_count, uint32_t frames, uint32_t node_count,
    HirariAudioGraphNodeInfo node_info,
    HirariAudioGraphProcessNode process_node);
extern "C" void* hirari_metronome_create(double sample_rate);
extern "C" void hirari_metronome_destroy(void* state);
extern "C" void hirari_metronome_set_sample_rate(void* state, double sample_rate);
extern "C" void hirari_metronome_reset(void* state);
extern "C" void hirari_metronome_set_enabled(void* state, bool enabled);
extern "C" bool hirari_metronome_is_enabled(const void* state);
extern "C" void hirari_metronome_process(
    void* state, float* left, float* right, uint32_t sample_count,
    uint64_t playhead, double sample_rate, double bpm);
extern "C" void* hirari_recording_capture_create();
extern "C" void hirari_recording_capture_destroy(void* state);
extern "C" bool hirari_recording_capture_start(
    void* state, const char* path, double sample_rate);
extern "C" void hirari_recording_capture_stop(void* state);
extern "C" bool hirari_recording_capture_write(
    const void* state, const float* left, const float* right, uint32_t frames);
extern "C" bool hirari_recording_capture_is_recording(const void* state);
extern "C" bool hirari_recording_capture_has_write_error(const void* state);
extern "C" bool hirari_recording_capture_has_overflowed(const void* state);
extern "C" uint64_t hirari_recording_capture_dropped_frames(const void* state);
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
extern "C" size_t hirari_undo_manager_apply_history(
    void* state, bool redo, uint64_t* output, size_t capacity);
extern "C" size_t hirari_undo_manager_abort_transaction(
    void* state, uint64_t* output, size_t capacity);
extern "C" size_t hirari_undo_manager_top_name(
    const void* state, bool redo, uint8_t* output, size_t capacity);
extern "C" size_t hirari_undo_manager_live_callback_count(const void* state);
extern "C" size_t hirari_undo_manager_copy_live_callbacks(
    const void* state, uint64_t* output, size_t capacity);
extern "C" void hirari_undo_manager_clear(void* state);
struct HirariAudioQuantizeChannelView {
    const float* samples = nullptr;
    uint64_t span = 0;
    double sourceFramesPerTimelineFrame = 1.0;
};
struct HirariAudioQuantizerMapPoint {
    uint64_t sourceSample = 0;
    uint64_t timelineSample = 0;
};
extern "C" {
typedef double HirariSamplesToBeatsCallback(void*, uint64_t);
typedef uint64_t HirariBeatsToSamplesCallback(void*, double);
bool hirari_audio_quantizer_build_group_envelope(
    const HirariAudioQuantizeChannelView* channels, size_t channel_count,
    uint64_t timeline_length, float* output, size_t output_capacity);
bool hirari_audio_quantizer_project_group_map(
    const HirariAudioQuantizerMapPoint* map, size_t map_count,
    uint64_t source_span, uint64_t timeline_span,
    double source_frames_per_timeline_frame,
    HirariWarpMarker* output, size_t output_capacity, size_t* output_count);
void* hirari_audio_quantizer_build_group_map(
    const float* const* inputs, uint32_t channels, uint64_t length,
    uint64_t timeline_start, double grid_beats, float strength, float swing,
    void* context, HirariSamplesToBeatsCallback* samples_to_beats,
    HirariBeatsToSamplesCallback* beats_to_samples);
void hirari_audio_quantizer_map_destroy(void* map);
size_t hirari_audio_quantizer_map_count(const void* map);
bool hirari_audio_quantizer_map_get(
    const void* map, size_t index, uint64_t* source, uint64_t* timeline);
}
extern "C" uint8_t hirari_plugin_format_for_path(const char* path);
extern "C" bool hirari_plugin_is_safe_candidate(
    const char* path, const char* expected_format);
extern "C" bool hirari_plugin_cache_key(
    const uint8_t* path, size_t path_size, uint8_t* output,
    size_t output_capacity, size_t* output_size);

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
extern "C" void hirari_midi_learn_remove_mapping(
    void* state, uint8_t cc, uint8_t channel);
extern "C" void hirari_midi_learn_handle_cc(
    const void* state, uint8_t channel, uint8_t cc, uint8_t value);
extern "C" void hirari_midi_learn_handle_cc14(
    const void* state, uint8_t channel, uint16_t controller, uint16_t value);
