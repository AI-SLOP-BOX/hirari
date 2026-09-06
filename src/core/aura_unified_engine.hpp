#pragma once

#include <vector>
#include <memory>
#include <atomic>
#include <chrono>
#include <string_view>
#include "audio_buffer.hpp"
#include "midi_buffer.hpp"
#include "engine_types.hpp"
#include "bridge_types.hpp"
#include "engine_types.hpp"
#include "utils/ring_buffer.hpp"
#include "concurrency/audio_task_manager.hpp"
#include "engine/process_graph.hpp"
#include "engine/pdc_manager.hpp"
#include "engine/tonal_sync.hpp"
#include "engine/tempo_map.hpp"
#include "engine/routing_engine.hpp"
#include "engine/undo_transaction_manager.hpp"
#include "engine/sidechain_manager.hpp"
#include "engine/track_freeze_manager.hpp"
#include "engine/bus_system.hpp"
#include "engine/macro_control_manager.hpp"
#include "engine/vca_manager.hpp"
#include "midi_learn_manager.hpp"
#include "engine/metronome.hpp"
#include "engine/automation_recorder.hpp"
#include "engine/midi_orchestrator.hpp"
#include "engine/mpe_manager.hpp"
#include "diagnostics/engine_diagnostics.hpp"
#include "diagnostics/forensic_journaler.hpp"
#include "io/audio_decoder.hpp"
#include "plugins/plugin_compatibility_registry.hpp"
#include <unordered_map>
#include <unordered_set>
#include <mutex>
#include <condition_variable>
#include <map>
#include <algorithm>
#include <iterator>
#include <cmath>
#include <cctype>
#include <fstream>
#include <filesystem>
#include <array>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

// Forward declarations
namespace Aura::DSP { struct ProcessContext; }
namespace Aura::Core::Engine { class ProcessGraph; }
namespace Aura::DSP::Analysis { class MasterMeter; }
namespace Aura::DSP::Effects { class MasterLimiter; }
#include "composition/neural_arrangement_kernel.hpp"

namespace Aura::Core::Video { class VideoSync; }

#include "log_buffer.hpp"
#include "diagnostics/engine_diagnostics.hpp"
#include "engine/script_manager.hpp"
#include "engine/macro_control_manager.hpp"
#include "engine/routing_engine.hpp"
#include "dsp/effects/atmospheric_processor_kernel.hpp"
#include "mixing/mastering_kernel.hpp"
#include "mixing/control_room.hpp"
#include "mixing/track_preset_store.hpp"
#include "engine/automation_lane_group.hpp"
#include "engine/tap_tempo.hpp"
#include "engine/audio_quantizer.hpp"
#include "midi/external_sync.hpp"
#include "composition/arrangement_edit_model.hpp"
#include <thread>
#include <future>

namespace Aura::Core::Engine {

class ScriptManager;
class Track;

namespace Mixing {
struct AestheticFeatures {
    float transientClarity = 1.0f;
    float dynamicComplexity = 0.0f;
};

struct QualitativeMetricEngine {
    static QualitativeMetricEngine& getInstance() {
        static QualitativeMetricEngine instance;
        return instance;
    }

    std::map<std::string, float> calculateScores(const AestheticFeatures& features) const {
        const float clarity = std::clamp(features.transientClarity, 0.0f, 1.0f);
        const float complexity = std::clamp(features.dynamicComplexity, 0.0f, 1.0f);
        return {
            {"transient_clarity", clarity},
            {"dynamic_complexity", complexity},
            {"balance", (clarity + (1.0f - std::fabs(complexity - 0.5f) * 2.0f)) * 0.5f},
            {"energy", complexity}
        };
    }
};

} // namespace Mixing

struct EngineConfig {
    float tempo;
    uint32_t timeSigNum;
    uint32_t timeSigDen;
    uint32_t sampleRate;
    uint32_t blockSize;
};

class AuraUnifiedEngine {
public:
    // One block-size contract shared by the native graph and every bridge
    // entry point. Keeping this in the public engine boundary prevents the
    // FFI from silently rejecting a block size that the configured engine can
    // process, or accepting a size the graph does not bound.
    static constexpr uint32_t kMaxAudioBlockSize = 16'384u;

    enum class BounceState : uint32_t {
        Idle = 0,
        Queued = 1,
        Rendering = 2,
        Completed = 3,
        Failed = 4,
        Cancelled = 5,
        Paused = 6,
    };

    struct AutomationPoint {
        double time = 0.0;
        float value = 0.0f;
        float curve = 0.0f;
    };

    static AuraUnifiedEngine& getInstance();
    AuraUnifiedEngine();
    ~AuraUnifiedEngine();

    void apply_config(const EngineConfig& cfg);
    void processBlock(::Aura::Core::AudioBuffer& output, uint32_t offset, uint32_t size);
    void processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples);
    // Shared-map transient quantization for phase-coherent multi-mic groups.
    // This is deliberately exposed at the engine boundary so offline editors
    // and realtime clients use the identical warp implementation.
    bool quantizeAudioGroup(float* const* buffers, uint32_t channels,
                            uint64_t length, float bpm, double sampleRate,
                            float strength = 1.0f, float swing = 0.0f) {
        if (!buffers || channels == 0 || channels > 32) return false;
        std::array<const float*, 32> inputs{};
        std::array<float*, 32> outputs{};
        for (uint32_t i = 0; i < channels; ++i) {
            inputs[i] = buffers[i];
            outputs[i] = buffers[i];
        }
        return ::Aura::Core::Engine::AudioQuantizer::quantizeGroup(
            inputs.data(), outputs.data(), channels, length, bpm, sampleRate,
            {strength, swing});
    }
    // Compatibility entry point for older native integrations.  Keep the
    // raw-buffer API routed through the same validated processing path as the
    // packaged application instead of maintaining a second DSP implementation.
    void renderBlock(float* outL, float* outR, uint32_t numSamples,
                     const EngineContext& context) {
        (void)context;
        if (outL == nullptr || outR == nullptr || numSamples == 0 ||
            numSamples > kMaxAudioBlockSize) {
            if (outL != nullptr && outR != nullptr && numSamples > 0 &&
                numSamples <= kMaxAudioBlockSize) {
                std::fill_n(outL, numSamples, 0.0f);
                std::fill_n(outR, numSamples, 0.0f);
            }
            return;
        }
        float* channels[2] = {outL, outR};
        processBlockDirect(channels, 2, numSamples);
    }
    void waitForAudioCallbacks() const noexcept;
    void prepareToPlay(double sr, uint32_t sz);
    void syncStructuralChanges();

    // Transport
    bool is_playing() const;
    float get_dsp_load() const noexcept { return m_dspLoad.load(std::memory_order_relaxed); }
    bool audio_range_overflowed() const noexcept {
        return m_audioRangeOverflow.load(std::memory_order_acquire);
    }
    uint32_t get_block_size() const noexcept;
    float get_latency_ms() const noexcept;
    float get_master_tail_ms() const noexcept;
    float get_track_latency_ms(uint32_t tid) const noexcept;
    float get_track_tail_ms(uint32_t tid) const noexcept;
    float get_track_pdc_compensation_ms(uint32_t tid) const noexcept;
    bool set_low_latency_mode(bool active) noexcept;
    bool low_latency_mode() const noexcept;
    bool set_tonal_scale(int32_t root, uint32_t scale_type) noexcept;
    bool is_note_in_tonal_scale(int32_t midi_note) const noexcept;
    int32_t tonal_root() const noexcept;
    uint32_t tonal_scale_type() const noexcept;
    void set_playing(bool playing);
    uint64_t get_playhead() const;
    void set_playhead(uint64_t pos);
    double get_playhead_beats() const;
    double samples_to_beats(uint64_t samples) const;
    uint64_t beats_to_samples(double beats) const;
    std::vector<double> get_tempo_events() const;
    std::vector<double> get_time_signature_events() const;
    void clear_time_signature_events();
    bool set_time_signature_event(double beat, uint8_t numerator, uint8_t denominator);
    void clear_tempo_events(double initial_bpm);
    bool set_tempo_event(double beat, double bpm, bool ramp);
    bool remove_tempo_event(double beat);
    bool move_tempo_event(double from_beat, double to_beat);
    float get_tempo() const;
    bool set_tempo(float bpm);
    bool add_marker(uint64_t sample, std::string_view name, uint32_t color) { return m_arrangement.upsertMarker({sample, std::string(name), color}); }
    bool remove_marker(uint64_t sample) { return m_arrangement.removeMarker(sample); }
    std::vector<uint64_t> marker_positions() const { std::vector<uint64_t> out; for (const auto& m : m_arrangement.markers()) out.push_back(m.sample); return out; }
    bool set_arranger_parts(std::vector<::Aura::Core::Composition::ArrangerPart> parts) { return m_arrangement.setArrangerParts(std::move(parts)); }
    std::vector<::Aura::Core::Composition::ArrangerPart> arranger_parts() const { return m_arrangement.arrangerParts(); }
    void tap_tempo(uint64_t timestamp_ms) noexcept { m_tapTempo.tap(timestamp_ms); const double bpm = m_tapTempo.bpm(); if (bpm > 0.0) m_tempo.store(static_cast<float>(bpm), std::memory_order_relaxed); }
    void clear_tap_tempo() noexcept { m_tapTempo.clear(); }
    double tapped_tempo() const noexcept { return m_tapTempo.bpm(); }
    void midi_clock_tick(uint64_t timestamp) noexcept { m_externalSync.onClockTick(timestamp); }
    uint64_t midi_clock_ticks() const noexcept { return m_externalSync.tickCount(); }
    uint64_t midi_clock_last_tick() const noexcept { return m_externalSync.lastTick(); }
    double midi_clock_rate() const noexcept { return m_externalSync.clockRate(); }
    void set_midi_clock_rate(double bpm) noexcept { m_externalSync.setClockRate(bpm); }
    bool set_master_gain(float value);
    bool add_control_room_speaker(std::string_view name, float gain = 1.0f) {
        return m_controlRoom.addSpeakerSet(std::string(name), gain);
    }
    void reset_control_room() { m_controlRoom.resetForProject(); }
    bool select_control_room_speaker(uint32_t index) noexcept {
        return m_controlRoom.selectSpeakerSet(index);
    }
    bool rename_control_room_speaker(uint32_t index, std::string_view name) {
        return m_controlRoom.renameSpeakerSet(index, std::string(name));
    }
    bool remove_control_room_speaker(uint32_t index) { return m_controlRoom.removeSpeakerSet(index); }
    bool set_control_room_speaker_gain(uint32_t index, float gain) { return m_controlRoom.setSpeakerGain(index, gain); }
    bool set_control_room_speaker_enabled(uint32_t index, bool enabled) { return m_controlRoom.setSpeakerEnabled(index, enabled); }
    bool upsert_control_room_cue(uint32_t id, float gain, bool enabled = true) { return m_controlRoom.upsertCueMix(id, gain, enabled); }
    bool remove_control_room_cue(uint32_t id) { return m_controlRoom.removeCueMix(id); }
    bool set_control_room_cue_enabled(uint32_t id, bool enabled) { return m_controlRoom.setCueMixEnabled(id, enabled); }
    float control_room_cue_gain(uint32_t id) const noexcept { return m_controlRoom.cueGain(id); }
    void set_control_room_dim(bool enabled) noexcept { m_controlRoom.setDim(enabled); }
    void set_control_room_talkback(bool enabled, float gain = 1.0f) noexcept {
        m_controlRoom.setTalkback(enabled, gain);
    }
    bool control_room_dimmed() const noexcept { return m_controlRoom.isDimmed(); }
    bool control_room_talkback_enabled() const noexcept { return m_controlRoom.talkbackEnabled(); }
    float control_room_monitor_gain() const noexcept { return m_controlRoom.monitorGain(); }
    bool control_room_validate() const noexcept { return m_controlRoom.validate(); }
    void process_control_room_monitor(float* left, float* right, const float* talkback,
                                      uint32_t frames) const noexcept {
        m_controlRoom.processMonitorWithTalkback(left, right, talkback, frames);
    }
    bool save_track_preset(std::string_view name, uint32_t track_id);
    bool apply_track_preset(std::string_view name, uint32_t track_id);
    bool remove_track_preset(std::string_view name);
    std::string plugin_compatibility_snapshot_json() const;
    void record_plugin_scan_failure(std::string_view id, std::string_view error);
    void record_plugin_crash(std::string_view id);
    void set_plugin_blacklisted(std::string_view id, bool value);
    bool add_automation_lane(uint32_t id) { return m_automationLanes.add(id); }
    bool set_automation_lane_linked(uint32_t id, bool linked) { return m_automationLanes.link(id, linked); }
    bool set_automation_lane_protected(uint32_t id, bool protectedLane) { return m_automationLanes.protect(id, protectedLane); }
    bool preview_automation_lane(uint32_t id, float value) { return m_automationLanes.preview(id, value); }
    float get_master_gain() const noexcept;
    bool set_track_volume(uint32_t tid, float value);
    float get_track_volume(uint32_t tid) const;
    bool set_track_pan(uint32_t tid, float value);
    bool set_track_mute(uint32_t tid, bool muted);
    bool set_track_solo(uint32_t tid, bool solo);
    bool set_track_solo_for_offline_render(uint32_t tid, bool solo);
    // Offline render target selection is control-side state. When active,
    // the audio graph keeps only the target and its upstream route closure,
    // then publishes the target output to the master buffer.
    bool set_offline_render_target(uint32_t tid);
    void clear_offline_render_target() noexcept;
    void set_offline_render_tail_seconds(float seconds) noexcept;
    void set_offline_render_options(bool pre_fader, bool include_inserts) noexcept;
    bool set_offline_render_range(uint64_t start_sample, uint64_t end_sample) noexcept;
    void clear_offline_render_range() noexcept;
    bool set_phase_invert(uint32_t tid, bool inverted);
    bool set_track_delay_samples(uint32_t tid, uint32_t samples);
    uint32_t get_track_delay_samples(uint32_t tid) const;
    bool set_track_armed(uint32_t tid, bool armed);
    double get_sample_rate() const { return m_sampleRate.load(std::memory_order_relaxed); }
    uint64_t get_audio_config_generation() const noexcept {
        return m_audioConfigGeneration.load(std::memory_order_acquire);
    }
    void set_loop(bool enabled);
    bool set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled);
    bool is_loop_enabled() const noexcept;
    uint64_t cycle_start() const noexcept;
    uint64_t cycle_end() const noexcept;
    void set_metronome_enabled(bool enabled) noexcept;
    bool is_metronome_enabled() const noexcept;
    void set_test_tone(bool enabled);
    bool bind_midi_cc_to_macro(uint8_t channel, uint8_t cc, uint32_t macro_index,
                               float minimum, float maximum, float curve,
                               bool pickup) noexcept;
    bool bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller, uint32_t macro_index,
                                 float minimum, float maximum, float curve,
                                 bool pickup) noexcept;
    void handle_midi_cc(uint8_t channel, uint8_t cc, uint8_t value) noexcept;
    void handle_midi_cc14(uint8_t channel, uint16_t controller, uint16_t value) noexcept;
    // Preview sampler bridge. The sample is replaced off the audio thread;
    // the callback only reads an immutable shared snapshot.
    void set_preview_sample(const float* samples, size_t count, double sourceRate);
    void trigger_preview_sample();
    void clear_preview_sample();

    // Tracks
    uint32_t add_track(uint32_t type);
    bool add_vca_group(uint32_t group_id, float gain);
    bool assign_track_to_vca(uint32_t track_id, uint32_t group_id);
    bool set_vca_group_gain(uint32_t group_id, float gain);
    float get_vca_track_gain(uint32_t track_id) const;
    rust::String get_vca_snapshot_json() const;
    void clear_vca_groups();
    bool remove_track(uint32_t id);
    uint32_t duplicate_track(uint32_t id);
    void new_project();
    bool set_track_name(uint32_t id, std::string_view name);
    uint32_t get_track_count() const;
    std::vector<std::shared_ptr<Track>> get_tracks_snapshot() const;
    // Freeze is a control/offline operation.  The engine owns the track
    // lifetime and serializes the render against the callback boundary
    // before publishing the immutable frozen buffer.
    bool freeze_track(uint32_t tid, uint64_t total_samples, uint32_t sample_rate);
    bool freeze_track_to_project_end(uint32_t tid, uint32_t sample_rate);
    bool freeze_track_to_file(uint32_t tid, std::string_view path,
                              uint64_t total_samples, uint32_t sample_rate);
    bool restore_track_freeze_from_file(uint32_t tid, std::string_view path,
                                        uint64_t total_samples, uint32_t sample_rate);
    bool set_track_freeze_cache_path(uint32_t tid, std::string_view path);
    bool unfreeze_track(uint32_t tid);
    bool is_track_frozen(uint32_t tid) const;
    rust::Slice<const float> get_track_peaks_l() const;
    rust::Slice<const float> get_track_peaks_r() const;
    Track& get_track(uint32_t id);

    // Regions & Plugins
    bool add_region(uint32_t trackId, std::string_view path, double start_beat);
    bool replace_region_audio(uint32_t trackId, uint32_t regionId, std::string_view path);
    bool add_plugin(uint32_t tid, uint32_t pluginType);
    std::vector<uint8_t> get_plugin_state(uint32_t tid, uint32_t pluginIndex) const;
    bool set_plugin_state(uint32_t tid, uint32_t pluginIndex,
                          const std::vector<uint8_t>& state);
    bool remove_plugin(uint32_t tid, uint32_t pluginIndex);
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex);
    bool add_sandboxed_plugin(uint32_t tid, std::string_view pluginPath);
    bool process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                        float* left, float* right, uint32_t frames);
    std::vector<uint8_t> process_sandboxed_plugin_midi_block(
        uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
        const uint8_t* midiData, uint32_t midiSize);
    // Control-thread maintenance. Returns the number of sandbox processes
    // successfully recovered during this pass.
    uint32_t maintain_sandboxed_plugins(bool autoRestart);
    bool retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    bool restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    bool reset_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    struct SandboxStatus {
        uint32_t trackId = 0;
        uint32_t pluginIndex = 0;
        bool alive = false;
        bool canRetry = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
        uint32_t mailboxOverruns = 0;
        uint32_t inputMidiTruncations = 0;
        uint32_t recoveryMode = 0;
    };
    std::vector<SandboxStatus> get_sandbox_statuses();
    // Control-thread diagnostic edge: number of AU processors that crossed
    // the consecutive-overrun watchdog since the previous poll.
    uint32_t take_watchdog_trips();
    uint64_t non_finite_plugin_samples() const;
    uint8_t get_last_sandbox_failure(uint32_t trackId) const;
    std::string get_last_sandbox_failure_text(uint32_t trackId) const;
    std::vector<std::string> get_sandbox_plugin_paths() const;
    std::vector<uint8_t> get_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex) const;
    bool set_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex,
                                  const std::vector<uint8_t>& state);
    uint8_t get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const;
    std::string get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const;
    bool set_plugin_parameter(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId, float value);
    // Internal automation/hydration path. It applies the value without
    // creating a user-facing Undo step for every generated control event.
    bool set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId, float value);
    float get_plugin_parameter(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    uint32_t get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const;
    bool has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const;
    uint64_t open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                       uintptr_t parent) const;
    bool close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                    uint64_t session) const;
    std::string get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    bool save_plugin_preset(uint32_t tid, uint32_t pluginIndex, std::string_view path) const;
    bool load_plugin_preset(uint32_t tid, uint32_t pluginIndex, std::string_view path);
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed);
    bool get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const;
    void set_macro_value(uint32_t macroIndex, float value);
    void set_preview_synth_engine(uint32_t engine);
    bool set_route(uint32_t sourceId, uint32_t destId, bool enabled);
    bool set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled);
    bool set_feedback_route(uint32_t sourceId, uint32_t destId, float gain, bool enabled);
    bool set_sidechain_link(uint32_t sourceId, uint32_t destId,
                            uint32_t pluginIndex, uint32_t tapPoint, bool enabled);
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const;

private:
    bool set_sidechain_link_internal(uint32_t sourceId, uint32_t destId,
                                     uint32_t pluginIndex, uint32_t tapPoint,
                                     bool enabled, bool recordUndo);

public:
    bool split_region(uint32_t tid, uint32_t rid, double splitBeat);
    bool remove_region(uint32_t tid, uint32_t rid);
    uint32_t duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample);
    bool move_region(uint32_t tid, uint32_t rid, double startBeat);
    bool set_region_gain(uint32_t tid, uint32_t rid, float gain);
    bool set_region_muted(uint32_t tid, uint32_t rid, bool muted);
    void clear_midi_notes();
    struct ScheduledMidiNote {
        uint32_t trackId = 0;
        uint8_t pitch = 60;
        uint8_t velocity = 100;
        uint64_t startSample = 0;
        uint64_t lengthSamples = 1;
    };
    bool replace_midi_notes(const std::vector<ScheduledMidiNote>& notes, bool recordUndo = true);
    std::vector<ScheduledMidiNote> midi_notes_snapshot() const;
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample);
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones);
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples);
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity,
                       uint64_t startSample, uint64_t lengthSamples);
    bool set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut);
    bool set_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start, uint64_t end,
                               float gain, uint64_t fadeIn, uint64_t fadeOut);
    bool clear_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start, uint64_t end);
    bool clear_region_range_edits(uint32_t tid, uint32_t rid);
    bool set_region_reverse(uint32_t tid, uint32_t rid, bool reverse);
    bool set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio);
    bool set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones);
    bool set_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                       double startSeconds, double endSeconds,
                                       double pitchOffsetCents, double formantOffsetCents);
    bool set_region_audio_note_anchor(uint32_t tid, uint32_t rid,
                                      double segmentStartSeconds, double positionSeconds,
                                      double pitchCents, double formantCents);
    bool clear_region_audio_note_segments(uint32_t tid, uint32_t rid);
    bool warp_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                        double segmentStartSeconds,
                                        double newStartSeconds, double newEndSeconds);
    bool remove_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                          double segmentStartSeconds);
    bool analyze_region_audio_note_segments(uint32_t tid, uint32_t rid, double sampleRate);
    bool set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count);
    bool set_region_locked(uint32_t tid, uint32_t rid, bool locked);
    bool set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group);
    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled);
    bool clear_region_processing(uint32_t tid, uint32_t rid);
    bool set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm);

    // Project
    bool saveProject(std::string_view path);
    bool loadProject(std::string_view path);
    bool bounce_project(std::string_view outputPath, uint32_t format);
    bool bounce_project_async(std::string_view outputPath, uint32_t format);
    // Lock-free status reads for UI/control threads. These never wait on the
    // audio callback or the bounce worker.
    float get_bounce_progress() const noexcept;
    uint32_t get_bounce_state() const noexcept;
    bool cancel_bounce() noexcept;
    bool pause_bounce() noexcept;
    bool resume_bounce() noexcept;
    bool generate_drum_fill(uint32_t track_id, uint32_t bar, float complexity);
    void set_show_video(bool s);
    void set_ascended_mode(bool enabled);
    void execute_restoration(uint32_t trackId, float intensity);
    bool execute_vocal_remover(uint32_t tid);
    void set_automation_record_mode(uint32_t mode);
    void set_automation_punch_range(uint64_t start, uint64_t end) noexcept;
    float get_track_correlation(uint32_t trackId) const;
    void sync_sharding_node(uint32_t node_id);
    void browser_preview(std::string_view path);
    rust::Vec<uint8_t> get_video_frame();
    uint64_t get_video_frame_revision() const noexcept;
    bool request_video_frame(double seconds);
    bool load_video(std::string_view path);
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid, uint32_t numPeaks);
    rust::Vec<float> get_region_audio_samples(uint32_t tid, uint32_t rid, uint32_t maxSamples);
    rust::Vec<float> get_region_audio_interleaved(uint32_t tid, uint32_t rid, uint32_t maxFrames);
    double get_region_sample_rate(uint32_t tid, uint32_t rid);
    uint32_t get_region_channel_count(uint32_t tid, uint32_t rid);

    // Automation
    bool set_automation_data(uint32_t tid, uint32_t param_id, const std::vector<AutomationPoint>& points);
    bool set_track_delay_automation(uint32_t tid, const std::vector<AutomationPoint>& points);
    bool push_command(::Aura::Core::CommandType type, uint32_t targetId, float value,
                      uint64_t timestamp, uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration);
    uint8_t get_last_command_error() const noexcept {
        return m_lastCommandError.load(std::memory_order_acquire);
    }

    // Spatial
    void set_spatial_mode(uint32_t tid, uint32_t mode);
    bool set_spatial_position(uint32_t tid, float x, float y, float z);
    bool set_hrtf_kernel(uint32_t tid, const std::vector<float>& left,
                         const std::vector<float>& right);
    bool clear_hrtf_kernel(uint32_t tid);

    // Analysis
    struct MeterData {
        float peakL = 0.0f;
        float peakR = 0.0f;
        float truePeakL = 0.0f;
        float truePeakR = 0.0f;
        float rmsL = 0.0f;
        float rmsR = 0.0f;
        float lufsShortTerm = 0.0f;
        float lufsIntegrated = 0.0f;
        float correlation = 0.0f;
        float balance = 0.0f;
    };
    MeterData getMasterMeterData() const;
    bool execute_auto_mixing();
    bool execute_auto_arrangement();
    void undo();
    void redo();
    void begin_undo_transaction(std::string_view name);
    bool end_undo_transaction();
    bool abort_undo_transaction();
    void clear_undo_history();
    uint32_t get_undo_count() const;
    uint32_t get_redo_count() const;
    rust::String get_project_layout_json() const;
    rust::String get_routing_snapshot_json() const;
    // Stable generation token for UI/CLI snapshots. Commands must echo this
    // value back through push_command; a changed layout makes the token stale.
    uint64_t get_project_generation() const;
    bool set_articulation_map(uint32_t trackId, uint32_t mapHash);
    bool set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb);
    bool set_project_scale(int32_t root, int32_t type);
    std::map<std::string, float> get_vibe_scores() const;
    void perform_autonomous_correction();
    
    // Phase 11: Neural Telemetry
    rust::Vec<Bridge::PlainClash> run_masking_analysis(uint32_t trackA);
    
    // Phase 14: Compositional Telemetry
    rust::Vec<Aura::Core::Composition::NeuralArrangementKernel::Section> run_structural_analysis();
    
    // Phase 16: Plugin Sovereignty
    void autonomous_plugin_audit();
    
    // Diagnostics & Health
    bool checkSystemHealth() const;

    void shutdown();

    // Telemetry access (RT-Safe)
    struct TelemetryData {
        float peaksL[256]; 
        float peaksR[256];
        uint32_t count = 0;
        float spectrum[512]; 
        float correlation;
        float dspLoad = 0.0f;
        uint32_t activeVoices = 0;
        Mixing::AestheticFeatures aesthetic; 
        uint64_t forensicAuditHash; // --- PHASE 50: CRYPTOGRAPHIC SOVEREIGNTY ---
        uint64_t version = 0; 
    };

    uint32_t get_active_telemetry_idx() const { return m_activeTelemetryIdx.load(std::memory_order_acquire); }
    const TelemetryData& get_telemetry(uint32_t idx) const { return m_telemetryBuffers[idx]; }
    
    void push_event(const ::Aura::Core::EngineEvent& e);
    bool pop_event(::Aura::Core::EngineEvent& e) const;

public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    uint32_t allocateTrackIdLocked() noexcept;
    uint32_t allocateRegionIdLocked() noexcept;
    // Audio engine state management (Linus-Grade: Cache-aligned cluster)
    struct alignas(64) AudioThreadState {
        Track* activeTracks[kMaxTracks];
        Track* trackMap[kMaxTrackMap]; // INDUSTRIAL: O(1) Track Lookup by ID
        uint32_t trackCount = 0;
        uint32_t stageCount = 0;
        uint64_t audioConfigGeneration = 0;
        
        // SBF-v6/Graph-v1
        ExecutionStage stages[32]; 
    };

    AudioThreadState m_audioStates[2];
    std::atomic<uint32_t> m_activeStateIdx{0};

    std::atomic<bool> m_isPlaying{false};
    // Incremented around the complete direct callback. Control-side object
    // reclamation must not rely on transport state alone: a CoreAudio
    // callback can still be in flight while stop/reconnect is being handled.
    std::atomic<uint32_t> m_audioCallbacksInFlight{0};
    std::atomic<bool> m_testTone{false};
    std::atomic<uint64_t> m_playhead{0};
    // Sticky diagnostic: a saturated sample position or invalid callback
    // range was observed. It is intentionally lock-free so the control/UI
    // side can report it without touching the audio callback.
    std::atomic<bool> m_audioRangeOverflow{false};
    std::atomic<bool> m_loopEnabled{false};
    std::atomic<uint64_t> m_cycleStart{0};
    std::atomic<uint64_t> m_cycleEnd{0};
    ::Aura::Core::Engine::Metronome m_metronome;
    std::atomic<float> m_tempo{120.0f};
    std::atomic<float> m_masterGain{1.0f};
    std::atomic<double> m_sampleRate{44100.0};
    // Generation zero is reserved for an unavailable/uninitialized engine.
    // The constructor publishes the valid default 44.1 kHz configuration as
    // generation one before any callback or command can observe it.
    std::atomic<uint64_t> m_audioConfigGeneration{1};
    std::atomic<bool> m_audioConfigTransition{false};
    
    // Lock-free queues
    struct Command {
        ::Aura::Core::CommandType type;
        uint32_t tid;
        float val;
        uint64_t ts;
        // Commands are valid only for the audio configuration snapshot from
        // which the UI/CLI observed them.  The callback rechecks this before
        // mutating a Track, preventing a queued command from crossing a
        // sample-rate or block-size transition.
        uint64_t expectedAudioGeneration{0};
    };

    enum class CommandPushError : uint8_t {
        None = 0,
        InvalidTarget = 1,
        StaleProjectGeneration = 2,
        StaleAudioGeneration = 3,
        QueueFull = 4,
    };

    std::atomic<uint8_t> m_lastCommandError{
        static_cast<uint8_t>(CommandPushError::None)};

    ::Aura::Core::RingBuffer<Command, 1024> m_commandQueue;
    mutable ::Aura::Core::RingBuffer<::Aura::Core::EngineEvent, 512> m_eventQueue;

    // Internal components (using PIMPL principles where possible)
    ::Aura::Core::AudioBuffer m_masterBuffer;
    ::Aura::Core::MidiBuffer m_blockMidi;
    // The native MIDI policy layer runs after timeline scheduling and before
    // preview/plugin dispatch, so articulation and MPE transformations affect
    // every downstream instrument consistently.
    ::Aura::Core::Engine::MIDIOrchestrator m_midiOrchestrator;
    struct PreviewVoice {
        bool active = false;
        uint8_t pitch = 60;
        float level = 0.0f;
        double phase = 0.0;
        uint64_t releaseAt = 0;
        uint64_t startedAt = 0;
    };
    PreviewVoice m_previewVoices[16]{};
    float m_previewFilterState = 0.0f;

    struct PreviewSample {
        std::vector<float> samples;
        double sourceRate = 44100.0;
    };
    // Append-only generations are owned by the control side. The audio
    // callback reads only this pointer and never touches a shared_ptr refcount.
    std::vector<std::unique_ptr<const PreviewSample>> m_previewSampleStorage;
    std::atomic<const PreviewSample*> m_previewSample{nullptr};
    std::atomic<bool> m_previewSampleTrigger{false};
    std::atomic<uint32_t> m_previewSynthEngine{0};
    double m_previewSamplePosition = 0.0;
    
    // DSP State (moved to pointer to decouple header)
    std::unique_ptr<::Aura::DSP::ProcessContext> m_context;
    ::Aura::Core::Concurrency::AudioTaskStealingScheduler m_threadPool;
    std::unique_ptr<::Aura::DSP::Analysis::MasterMeter> m_analyzer;
    std::unique_ptr<::Aura::DSP::Effects::MasterLimiter> m_masterLimiter;
    
    // Aesthetic Sovereignty
    // INDUSTRIAL: Lock-Free Atomic State (Trivially Copyable)
    std::atomic<Mixing::AestheticFeatures> m_vibeData{ Mixing::AestheticFeatures{} };
    std::atomic<uint32_t> m_vibeVersion{0}; 

    TelemetryData m_telemetryBuffers[2];
    std::atomic<uint32_t> m_activeTelemetryIdx{0};
    
    std::atomic<bool> m_isShutdown{false};
    // Project hydration mutates tracks, routing and several process-wide
    // managers as one control-plane transaction.  Reject a concurrent load
    // instead of allowing two documents to interleave partial state.
    std::atomic<bool> m_projectHydrationActive{false};
    std::atomic<bool> m_offlineRenderActive{false};
    std::atomic<uint64_t> m_offlineRenderOwner{0};
    std::atomic<bool> m_offlineTargetActive{false};
    std::atomic<uint32_t> m_offlineTargetId{0};
    std::atomic<uint32_t> m_offlineRenderTailMillis{0};
    std::atomic<bool> m_offlineRenderPreFader{false};
    std::atomic<bool> m_offlineRenderIncludeInserts{true};
    std::atomic<bool> m_offlineRenderRangeActive{false};
    std::atomic<uint64_t> m_offlineRenderStartSample{0};
    std::atomic<uint64_t> m_offlineRenderEndSample{0};
    std::atomic<bool> m_bounceRunning{false};
    std::atomic<float> m_bounceProgress{0.0f};
    std::atomic<uint32_t> m_bounceState{static_cast<uint32_t>(BounceState::Idle)};
    std::atomic<bool> m_bounceCancelRequested{false};
    std::atomic<bool> m_bouncePauseRequested{false};
    // Serializes cancellation with the final temporary-file publish so a
    // cancel cannot race a stale render into the destination path.
    mutable std::mutex m_bouncePublishMutex;
    // Serializes render thread retirement with a subsequent start. The
    // worker publishes Failed/Completed before its wrapper flips
    // m_bounceRunning, so callers must be able to join a terminal worker
    // before accepting the next render.
    mutable std::mutex m_bounceLifecycleMutex;
    std::thread m_bounceThread;
    std::vector<std::shared_ptr<Track>> m_tracks;
    std::vector<std::shared_ptr<Track>> m_deferredDestructionTracks;
    mutable std::mutex m_tracksMutex;

    struct SidechainRoute {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        uint32_t pluginIndex = 0;
        uint32_t tapPoint = 2;
    };
    std::vector<SidechainRoute> m_sidechainRoutes;

    // UI/control thread publishes a complete immutable snapshot; the audio
    // thread only reads the active slot and never waits for a mutex.
    std::vector<ScheduledMidiNote> m_midiNoteSnapshots[2];
    std::atomic<uint32_t> m_activeMidiSnapshot{0};
    mutable std::mutex m_midiNotesMutex;

    std::atomic<float> m_dspLoad{0.0f};
    std::atomic<uint32_t> m_diagnosticBlockCounter{0};
    // Zero is reserved as an invalid/unset region identity in serialized
    // projects and FFI calls. All runtime-created regions start at one.
    std::atomic<uint32_t> m_regionIdCounter{1};
    std::atomic<uint32_t> m_trackIdCounter{0};
    std::atomic<bool> m_showVideo{false};
    std::atomic<bool> m_isAscended{false};
    std::atomic<uint32_t> m_autoRecordMode{0};
    std::atomic<int32_t> m_rootNote{0}, m_scaleType{0};
    std::atomic<uint32_t> m_articulationMapHash{0};
    // SBF-v6 Workspace Sovereignty
    struct Workspace {
        std::atomic<float> zoomX{1.0f}, zoomY{1.0f};
        std::atomic<uint64_t> scrollPos{0};
        std::atomic<uint32_t> focusedTrackId{0};
    } m_workspace;


    // Phase 9: Forensic Peak Cache (Zero-Wait UI)
    struct WaveformCache {
        float* mappedData = nullptr;
        size_t mappedSize = 0;
        int fd = -1;
        std::atomic<bool> valid{false};
        uint32_t sampleCount = 0;
        std::string filepath;

        ~WaveformCache();
        
        WaveformCache() = default;
        WaveformCache(const WaveformCache&) = delete;
        WaveformCache& operator=(const WaveformCache&) = delete;
        
        WaveformCache(WaveformCache&& other) noexcept {
            mappedData = other.mappedData;
            mappedSize = other.mappedSize;
            fd = other.fd;
            valid.store(other.valid.load());
            sampleCount = other.sampleCount;
            filepath = std::move(other.filepath);
            
            other.mappedData = nullptr;
            other.mappedSize = 0;
            other.fd = -1;
        }
        
        WaveformCache& operator=(WaveformCache&& other) noexcept {
            if (this != &other) {
                cleanup();
                mappedData = other.mappedData;
                mappedSize = other.mappedSize;
                fd = other.fd;
                valid.store(other.valid.load());
                sampleCount = other.sampleCount;
                filepath = std::move(other.filepath);
                
                other.mappedData = nullptr;
                other.mappedSize = 0;
                other.fd = -1;
            }
            return *this;
        }

    public:
        void cleanup();
    };
    std::unordered_map<uint32_t, WaveformCache> m_peakCache;
    // A region can be rebuilt while an older project generation is still
    // draining. Store the generation so an old worker cannot erase a newer
    // build registration.
    std::unordered_map<uint32_t, uint64_t> m_peakBuilds;
    std::atomic<uint64_t> m_waveformGeneration{0};
    mutable std::mutex m_cacheMutex;

    std::unique_ptr<ProcessGraph> m_processGraph;
    // PDC is project/session state. It must not be shared through the legacy
    // process-wide compatibility singleton.
    PDCManager m_pdcManager;
    TonalSync m_tonalSync;
    UndoTransactionManager m_undoManager;
    TempoMap m_tempoMap;
    RoutingEngine m_routingEngine;
    SidechainManager m_sidechainManager;
    BusSystem m_busSystem;
    MacroControlManager m_macroControlManager;
    ::Aura::Core::MidiLearnManager m_midiLearnManager;
    ::Aura::Core::Diagnostics::EngineDiagnostics m_engineDiagnostics;
    ::Aura::Core::Diagnostics::ForensicJournaler m_forensicJournaler;
    ::Aura::Core::IO::AudioDecoderManager m_audioDecoderManager;
    std::atomic<bool> m_graphDirty{true};

    // Engine Infrastructure
    std::unique_ptr<ScriptManager> m_scriptManager;
    
    // RT-Safe Pre-allocated Kernels
    ::Aura::Core::DSP::Effects::AtmosphericProcessorKernel m_atmosphericEngine;
    ::Aura::Core::Mixing::MasteringKernel m_masteringKernel;
    ::Aura::Core::Mixing::ControlRoom m_controlRoom;
    ::Aura::Core::Mixing::TrackPresetStore m_trackPresets;
    ::Aura::Core::Plugins::PluginCompatibilityRegistry m_pluginCompatibility;
    ::Aura::Core::Engine::AutomationLaneGroup m_automationLanes;
    ::Aura::Core::Engine::TapTempo m_tapTempo;
    ::Aura::Core::MIDI::ExternalSync m_externalSync;
    ::Aura::Core::Composition::ArrangementEditModel m_arrangement;
    mutable std::atomic<bool> m_ecoModeLogTriggered{false};
    mutable std::atomic<bool> m_videoSyncLogTriggered{false};

    // Phase 10: Sovereign Recovery
    void startRecoveryOrchestrator();
    std::thread m_recoveryThread;
    std::atomic<bool> m_recoveryRunning{false};
    std::condition_variable m_recoveryWake;
    std::mutex m_recoveryWakeMutex;
    // Manual health polling and the background recovery loop share the same
    // sandbox host objects. Serialize maintenance so stop/restart cannot race
    // with another poll on the same worker or IPC mapping.
    mutable std::mutex m_sandboxMaintenanceMutex;
    struct SandboxObservedStatus {
        bool alive = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
    };
    std::unordered_map<uint64_t, SandboxObservedStatus> m_sandboxObserved;
};

} // namespace Aura::Core::Engine
