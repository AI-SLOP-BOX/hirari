#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdlib>
#include <string>
#include <memory>
#include <mutex>
#include "rust/cxx.h"
#include "aura_unified_engine.hpp"
#include "bridge_types.hpp"
#include "../io/wav_loader_utils.hpp"

#if defined(__APPLE__)
#include "driver/mac_audio_driver_host.hpp"
#include "driver/mac_midi_device_host.hpp"
#endif

namespace Aura::Core::BridgeFFI {

#if defined(__APPLE__)
using AudioDriverHost = ::Aura::Core::Driver::MacAudioDriverHost;
#else
// Non-macOS builds currently have no native audio-device implementation.
// Keep the bridge constructible for offline/UI use, but never report a fake
// running device or fabricate callback/peak data.
class UnavailableAudioDriver {
public:
    bool start() noexcept { m_error = "native audio backend unavailable"; return false; }
    void stop() noexcept { m_running = false; }
    bool is_running() const noexcept { return false; }
    bool isSilentFallback() const noexcept { return true; }
    void try_reconnect() const noexcept { m_error = "native audio backend unavailable"; }
    float output_peak() const noexcept { return 0.0f; }
    uint64_t callback_count() const noexcept { return 0; }
    uint64_t dropped_input_blocks() const noexcept { return 0; }
    const char* last_error() const noexcept { return m_error.c_str(); }
    const char* status() const noexcept { return "unavailable"; }
private:
    mutable std::string m_error = "native audio backend unavailable";
    bool m_running = false;
};

using AudioDriverHost = UnavailableAudioDriver;
#endif

struct BridgeScoreGlyph {
    uint32_t type;
    float x, y;
};

/**
 * @class AudioEngine
 * @brief Industrial-grade Audio Engine wrapper for Aura Studio Pro.
 */
class AudioEngine {
public:
    AudioEngine();
    explicit AudioEngine(bool startDevice);
    ~AudioEngine();

    // Explicit lifecycle API for CLI, preview, and offline sessions. The
    // default constructor retains legacy product startup behavior.
    bool start_audio_device() const;

    void set_playing(bool playing) const {
        (void)try_set_playing(playing);
    }
    bool try_set_playing(bool playing) const {
        if (!m_engine) return false;
        if (playing && (!m_driver || !m_driver->is_running())) return false;
        m_engine->set_playing(playing);
        return true;
    }
    void set_loop(bool enabled) const { if (m_engine) m_engine->set_loop(enabled); }
    bool set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled) const { return m_engine && m_engine->set_cycle_range(start_sample, end_sample, enabled); }
    bool is_loop_enabled() const { return m_engine && m_engine->is_loop_enabled(); }
    uint64_t cycle_start() const { return m_engine ? m_engine->cycle_start() : 0; }
    uint64_t cycle_end() const { return m_engine ? m_engine->cycle_end() : 0; }
    void set_metronome_enabled(bool enabled) const {
        if (m_engine) m_engine->set_metronome_enabled(enabled);
    }
    bool is_metronome_enabled() const {
        return m_engine && m_engine->is_metronome_enabled();
    }
    bool is_playing() const { return m_engine && m_engine->is_playing(); }
    void set_playhead(uint64_t pos) const { if (m_engine) m_engine->set_playhead(pos); }
    uint64_t get_playhead() const { return m_engine ? m_engine->get_playhead() : 0; }
    double samples_to_beats(uint64_t samples) const { return m_engine ? m_engine->samples_to_beats(samples) : 0.0; }
    uint64_t beats_to_samples(double beats) const { return m_engine ? m_engine->beats_to_samples(beats) : 0; }
    rust::Vec<double> get_tempo_events() const {
        rust::Vec<double> result;
        if (!m_engine) return result;
        const auto values = m_engine->get_tempo_events();
        result.reserve(values.size());
        for (const double value : values) result.push_back(value);
        return result;
    }
    rust::Vec<double> get_time_signature_events() const {
        rust::Vec<double> result;
        if (!m_engine) return result;
        const auto values = m_engine->get_time_signature_events();
        result.reserve(values.size());
        for (const double value : values) result.push_back(value);
        return result;
    }
    bool set_time_signature_event(double beat, uint8_t numerator, uint8_t denominator) const {
        return m_engine && m_engine->set_time_signature_event(beat, numerator, denominator);
    }
    void clear_time_signature_events() const { if (m_engine) m_engine->clear_time_signature_events(); }
    void clear_tempo_events(double initial_bpm) const { if (m_engine) m_engine->clear_tempo_events(initial_bpm); }
    bool set_tempo_event(double beat, double bpm, bool ramp) const {
        return m_engine && m_engine->set_tempo_event(beat, bpm, ramp);
    }
    bool remove_tempo_event(double beat) const { return m_engine && m_engine->remove_tempo_event(beat); }
    bool move_tempo_event(double from_beat, double to_beat) const {
        return m_engine && m_engine->move_tempo_event(from_beat, to_beat);
    }
    float get_tempo() const { return m_engine ? m_engine->get_tempo() : 120.0f; }
    bool set_tempo(float bpm) const { return m_engine && m_engine->set_tempo(bpm); }
    bool set_track_volume(uint32_t tid, float value) const {
        return m_engine && m_engine->set_track_volume(tid, value);
    }
    float get_track_volume(uint32_t tid) const {
        return m_engine ? m_engine->get_track_volume(tid) : -1.0f;
    }
    bool set_master_gain(float value) const {
        return m_engine && m_engine->set_master_gain(value);
    }
    void clear_undo_history() const {
        if (m_engine) m_engine->clear_undo_history();
    }
    float get_master_gain() const noexcept {
        return m_engine ? m_engine->get_master_gain() : 1.0f;
    }
    bool set_track_delay_samples(uint32_t tid, uint32_t samples) const {
        return m_engine && m_engine->set_track_delay_samples(tid, samples);
    }
    uint32_t get_track_delay_samples(uint32_t tid) const {
        return m_engine ? m_engine->get_track_delay_samples(tid) : 0u;
    }
    bool set_track_pan(uint32_t tid, float value) const {
        return m_engine && m_engine->set_track_pan(tid, value);
    }
    bool set_track_mute(uint32_t tid, bool muted) const {
        return m_engine && m_engine->set_track_mute(tid, muted);
    }
    bool set_track_solo(uint32_t tid, bool solo) const {
        return m_engine && m_engine->set_track_solo(tid, solo);
    }
    bool set_track_solo_for_offline_render(uint32_t tid, bool solo) const {
        return m_engine && m_engine->set_track_solo_for_offline_render(tid, solo);
    }
    bool set_offline_render_target(uint32_t tid) const {
        return m_engine && m_engine->set_offline_render_target(tid);
    }
    void clear_offline_render_target() const noexcept {
        if (m_engine) m_engine->clear_offline_render_target();
    }
    void set_offline_render_tail_seconds(float seconds) const noexcept {
        if (m_engine) m_engine->set_offline_render_tail_seconds(seconds);
    }
    void set_offline_render_options(bool preFader, bool includeInserts) const noexcept {
        if (m_engine) m_engine->set_offline_render_options(preFader, includeInserts);
    }
    bool set_phase_invert(uint32_t tid, bool inverted) const {
        return m_engine && m_engine->set_phase_invert(tid, inverted);
    }
    bool set_track_armed(uint32_t tid, bool armed) const {
        return m_engine && m_engine->set_track_armed(tid, armed);
    }
    bool is_audio_device_ready() const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        const bool ready = m_driver && m_driver->is_running();
        if (!ready && m_engine && m_engine->is_playing()) {
            // Device loss is a transport safety boundary, not merely a UI
            // status change. Stop the graph even when no UI poll is running.
            m_engine->set_playing(false);
        }
        return ready;
    }
    bool is_silent_fallback() const noexcept {
#if defined(__APPLE__)
        if (!m_driver) return true;
        const char* status = m_driver->status();
        return status == nullptr || std::strcmp(status, "unavailable") == 0 ||
               std::strcmp(status, "start-failed") == 0;
#else
        return !m_driver || m_driver->isSilentFallback();
#endif
    }
    rust::String audio_driver_status() const {
        if (!m_driver) return rust::String("unavailable");
#if defined(__APPLE__)
        return rust::String(m_driver->status());
#else
        if (m_driver->isSilentFallback()) return rust::String("silent-fallback");
#endif
        return rust::String("stopped");
    }
    int32_t audio_driver_error_code() const noexcept {
#if defined(__APPLE__)
        return m_driver ? m_driver->last_error_code() : 0;
#else
        return 0;
#endif
    }
#if !defined(__APPLE__)
    bool has_audio_device_error() const noexcept {
        return m_driver && m_driver->last_error()[0] != '\0';
    }
    rust::String audio_device_error() const {
        return rust::String(m_driver ? m_driver->last_error() : "audio driver unavailable");
    }
#endif
    float get_audio_output_peak() const {
        return m_driver ? m_driver->output_peak() : 0.0f;
    }
    uint64_t get_audio_callback_count() const { return m_driver ? m_driver->callback_count() : 0; }
    uint64_t get_dropped_input_blocks() const { return m_driver ? m_driver->dropped_input_blocks() : 0; }
    rust::Vec<float> poll_audio_input() const;
    void try_reconnect_audio_device() const {
#if defined(__APPLE__)
        const char* isolated = std::getenv("AURA_NATIVE_TEST_ISOLATION");
        if (isolated && std::strcmp(isolated, "1") == 0) return;
#endif
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) {
            m_driver->try_reconnect();
            if (!m_driver->is_running() && m_engine) m_engine->set_playing(false);
        }
    }
    double get_sample_rate() const { return m_engine ? m_engine->get_sample_rate() : 44100.0; }
    rust::String list_audio_devices_json() const {
#if defined(__APPLE__)
        return rust::String(m_driver ? m_driver->list_devices_json() : "[]");
#else
        return rust::String("[]");
#endif
    }
    rust::String list_midi_devices_json() const {
        return rust::String(::Aura::Core::Driver::list_core_midi_devices_json());
    }
    bool send_midi_message(uint32_t uniqueId, rust::Slice<const uint8_t> data) const {
        return ::Aura::Core::Driver::send_core_midi_message(uniqueId, data.data(), data.size());
    }
    bool start_midi_input() const { return ::Aura::Core::Driver::start_core_midi_input(); }
    void stop_midi_input() const { ::Aura::Core::Driver::stop_core_midi_input(); }
    rust::String poll_midi_input_json() const { return rust::String(::Aura::Core::Driver::poll_core_midi_input_json()); }
    bool select_audio_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) const {
#if defined(__APPLE__)
        return m_driver && m_driver->select_device(deviceId, sampleRate, bufferSize);
#else
        (void)deviceId; (void)sampleRate; (void)bufferSize; return false;
#endif
    }
    uint64_t get_audio_config_generation() const {
        return m_engine ? m_engine->get_audio_config_generation() : 0;
    }
    uint32_t get_block_size() const { return m_engine ? m_engine->get_block_size() : 0; }
    float get_latency_ms() const { return m_engine ? m_engine->get_latency_ms() : 0.0f; }
    void set_test_tone(bool enabled) const { if (m_engine) m_engine->set_test_tone(enabled); }
    void set_preview_sample(rust::Slice<const float> samples, double sourceRate) const {
        if (m_engine) m_engine->set_preview_sample(samples.data(), samples.size(), sourceRate);
    }
    /// Processes one bounded stereo block through the C++ graph. Rust must
    /// not maintain a second production audio graph.
    void process_audio_block(rust::Slice<float> left, rust::Slice<float> right) const noexcept {
        // The native graph is prepared for bounded realtime blocks. Reject
        // oversized FFI slices before narrowing to uint32_t; otherwise a
        // hostile or malformed caller could wrap the frame count and reach
        // the DSP with a different size than the slice actually contains.
        if (!m_engine || left.empty() || right.empty() || left.size() != right.size() ||
            left.size() > ::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize) {
            return;
        }
        float* channels[2] = {left.data(), right.data()};
        if (m_engine) m_engine->processBlockDirect(channels, 2, static_cast<uint32_t>(left.size()));
    }
    void trigger_preview_sample() const { if (m_engine) m_engine->trigger_preview_sample(); }
    void clear_preview_sample() const { if (m_engine) m_engine->clear_preview_sample(); }
    void apply_config(float tempo, uint32_t sr, uint32_t bs) const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        (void)apply_config_locked(tempo, sr, bs);
    }

    // Configuration is a control-plane transaction.  Keeping the driver
    // transition and graph rebuild under the same session lock prevents two
    // UI/CLI callers from interleaving a device reconfigure with a second
    // prepareToPlay and publishing a generation that describes neither one.
    bool apply_config_locked(float tempo, uint32_t sr, uint32_t bs) const {
        // Keep the legacy void ABI, but make every entrypoint obey the same
        // configuration contract as try_apply_config. Invalid direct callers
        // must not quiesce the callback or advance the audio generation.
        if (!m_engine || !std::isfinite(tempo) || tempo <= 0.0f || tempo > 999.0f ||
            sr < 8'000u || sr > 384'000u || bs == 0u || bs > 16'384u) {
            return false;
        }
        // A configuration transition is a stop-the-world control operation.
        // Do not silently stop a live transport here: callers must perform
        // the explicit transport transition first, and the check is made
        // while m_configMutex is held so a concurrent start cannot slip
        // between the UI preflight and driver reconfiguration.
        if (m_engine->is_playing()) return false;
        ::Aura::Core::Engine::EngineConfig cfg;
        cfg.tempo = tempo; cfg.sampleRate = sr; cfg.blockSize = bs;
#if defined(__APPLE__)
        // The graph and CoreAudio must be rebuilt as one transaction. Updating
        // only the graph leaves the device callback on the previous format.
        const char* isolated = std::getenv("AURA_NATIVE_TEST_ISOLATION");
        const bool offlineIsolation = isolated && std::strcmp(isolated, "1") == 0;
        if (!offlineIsolation) {
            if (!m_driver || !m_driver->reconfigure(static_cast<double>(sr), bs)) {
                return false;
            }
        } else {
            // Integration tests deliberately detach from the process-wide
            // CoreAudio callback. Keep the native graph configurable offline
            // so sandbox/FFI tests do not accidentally require a physical
            // device, while the product path still requires a real driver.
            m_engine->waitForAudioCallbacks();
            m_engine->apply_config(cfg);
        }
#else
        // Device/sample-rate/block-size changes are control-thread events.
        // Quiesce any callback that still owns the previous audio context
        // before rebuilding track processors and PDC state.
        m_engine->waitForAudioCallbacks();
        m_engine->apply_config(cfg);
#endif
        return true;
    }
    bool try_apply_config(float tempo, uint32_t sr, uint32_t bs) const {
        if (!m_engine || !std::isfinite(tempo) || tempo <= 0.0f || tempo > 999.0f ||
            sr < 8'000u || sr > 384'000u || bs == 0u || bs > 16'384u) return false;
        std::lock_guard<std::mutex> lock(m_configMutex);
        const uint64_t before = m_engine->get_audio_config_generation();
        const double previousSampleRate = m_engine->get_sample_rate();
        const uint32_t previousBlockSize = m_engine->get_block_size();
        if (!apply_config_locked(tempo, sr, bs)) return false;
#if defined(__APPLE__)
        const char* isolated = std::getenv("AURA_NATIVE_TEST_ISOLATION");
        const bool offlineIsolation = isolated && std::strcmp(isolated, "1") == 0;
        if (!offlineIsolation && (!m_driver || !m_driver->is_running())) {
            // start_locked prepares the graph before opening the device. If
            // CoreAudio rejects the requested format, restore the last graph
            // configuration instead of leaving the engine/device split.
            if (previousSampleRate > 0.0 && previousBlockSize > 0) {
                m_engine->prepareToPlay(previousSampleRate, previousBlockSize);
            }
            return false;
        }
#endif
        return m_engine->get_audio_config_generation() != before &&
               std::abs(m_engine->get_sample_rate() - static_cast<double>(sr)) < 0.5 &&
               m_engine->get_block_size() == bs;
    }

    uint32_t add_track(uint32_t type) const { return m_engine ? m_engine->add_track(type) : 0; }
    bool add_vca_group(uint32_t groupId, float gain) const { return m_engine && m_engine->add_vca_group(groupId, gain); }
    bool assign_track_to_vca(uint32_t trackId, uint32_t groupId) const { return m_engine && m_engine->assign_track_to_vca(trackId, groupId); }
    bool set_vca_group_gain(uint32_t groupId, float gain) const { return m_engine && m_engine->set_vca_group_gain(groupId, gain); }
    float get_vca_track_gain(uint32_t trackId) const { return m_engine ? m_engine->get_vca_track_gain(trackId) : 1.0f; }
    rust::String get_vca_snapshot_json() const { return m_engine ? m_engine->get_vca_snapshot_json() : rust::String("[]"); }
    void clear_vca_groups() const { if (m_engine) m_engine->clear_vca_groups(); }
    bool remove_track(uint32_t id) const { return m_engine && m_engine->remove_track(id); }
    bool set_track_name(uint32_t id, rust::Str name) const { return m_engine && m_engine->set_track_name(id, std::string_view(name.data(), name.size())); }
    void new_project() const { if (m_engine) m_engine->new_project(); }
    bool set_low_latency_mode(bool active) const { return m_engine && m_engine->set_low_latency_mode(active); }
    bool low_latency_mode() const { return m_engine && m_engine->low_latency_mode(); }
    bool set_tonal_scale(int32_t root, uint32_t scale_type) const { return m_engine && m_engine->set_tonal_scale(root, scale_type); }
    bool is_note_in_tonal_scale(int32_t midi_note) const { return m_engine && m_engine->is_note_in_tonal_scale(midi_note); }
    int32_t tonal_root() const { return m_engine ? m_engine->tonal_root() : 0; }
    uint32_t tonal_scale_type() const { return m_engine ? m_engine->tonal_scale_type() : 0; }
    uint32_t duplicate_track(uint32_t id) const { return m_engine ? m_engine->duplicate_track(id) : 0; }
    bool freeze_track(uint32_t tid, uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track(tid, totalSamples, sampleRate);
    }
    bool freeze_track_to_file(uint32_t tid, rust::Str path, uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track_to_file(
            tid, std::string_view(path.data(), path.size()), totalSamples, sampleRate);
    }
    bool set_track_freeze_cache_path(uint32_t tid, rust::Str path) const {
        return m_engine && m_engine->set_track_freeze_cache_path(
            tid, std::string_view(path.data(), path.size()));
    }
    bool restore_track_freeze_from_file(uint32_t tid, rust::Str path,
                                        uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->restore_track_freeze_from_file(
            tid, std::string_view(path.data(), path.size()), totalSamples, sampleRate);
    }
    bool unfreeze_track(uint32_t tid) const {
        return m_engine && m_engine->unfreeze_track(tid);
    }
    bool is_track_frozen(uint32_t tid) const {
        return m_engine && m_engine->is_track_frozen(tid);
    }
    bool freeze_track_to_project_end(uint32_t tid, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track_to_project_end(tid, sampleRate);
    }
    bool add_region(uint32_t tid, rust::Str path, double startBeat) const {
        return m_engine && m_engine->add_region(tid, std::string_view(path.data(), path.size()), startBeat);
    }
    bool replace_region_audio(uint32_t tid, uint32_t rid, rust::Str path) const {
        return m_engine && m_engine->replace_region_audio(tid, rid, std::string_view(path.data(), path.size()));
    }
    
    bool add_plugin(uint32_t tid, uint32_t pluginType) const { return m_engine && m_engine->add_plugin(tid, pluginType); }
    rust::Vec<uint8_t> get_plugin_state(uint32_t tid, uint32_t pluginIndex) const {
        rust::Vec<uint8_t> out;
        if (!m_engine) return out;
        for (const auto byte : m_engine->get_plugin_state(tid, pluginIndex)) out.push_back(byte);
        return out;
    }
    bool set_plugin_state(uint32_t tid, uint32_t pluginIndex,
                          rust::Slice<const uint8_t> state) const {
        if (!m_engine) return false;
        std::vector<uint8_t> bytes;
        bytes.reserve(state.size());
        for (const auto byte : state) bytes.push_back(byte);
        return m_engine->set_plugin_state(tid, pluginIndex, bytes);
    }
    bool remove_plugin(uint32_t tid, uint32_t pluginIndex) const { return m_engine && m_engine->remove_plugin(tid, pluginIndex); }
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) const { return m_engine && m_engine->move_plugin(tid, fromIndex, toIndex); }
    bool add_sandboxed_plugin(uint32_t tid, rust::Str path) const {
        return m_engine && m_engine->add_sandboxed_plugin(tid, std::string_view(path.data(), path.size()));
    }
    bool process_sandboxed_plugin_block(uint32_t tid, uint32_t sandbox_index,
                                        rust::Slice<float> left, rust::Slice<float> right) const {
        if (!m_engine || left.empty() || left.size() != right.size()) return false;
        return m_engine->process_sandboxed_plugin_block(
            tid, sandbox_index, left.data(), right.data(), static_cast<uint32_t>(left.size()));
    }
    rust::Vec<uint8_t> process_sandboxed_plugin_midi_block(
        uint32_t tid, uint32_t sandbox_index, uint32_t frames,
        rust::Slice<const uint8_t> midi_data) const {
        rust::Vec<uint8_t> result;
        if (!m_engine || midi_data.empty()) return result;
        const auto output = m_engine->process_sandboxed_plugin_midi_block(
            tid, sandbox_index, frames, midi_data.data(), static_cast<uint32_t>(midi_data.size()));
        for (const auto byte : output) result.push_back(byte);
        return result;
    }
    uint32_t maintain_sandboxed_plugins(bool autoRestart) const {
        return m_engine ? m_engine->maintain_sandboxed_plugins(autoRestart) : 0;
    }
    uint32_t take_watchdog_trips() const {
        return m_engine ? m_engine->take_watchdog_trips() : 0;
    }
    uint64_t non_finite_plugin_samples() const {
        return m_engine ? m_engine->non_finite_plugin_samples() : 0;
    }
    bool retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine && m_engine->retry_sandboxed_plugin(trackId, sandboxIndex);
    }
    bool restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine && m_engine->restart_sandboxed_plugin(trackId, sandboxIndex);
    }
    rust::Vec<uint32_t> get_sandbox_statuses() const {
        rust::Vec<uint32_t> result;
        if (!m_engine) return result;
        result.push_back(Plugins::SandboxProtocol::kStatusHeaderV9);
        if (!m_engine) return result;
        for (const auto& status : m_engine->get_sandbox_statuses()) {
            result.push_back(status.trackId);
            result.push_back(status.pluginIndex);
            result.push_back(status.alive ? 1u : 0u);
            result.push_back(status.canRetry ? 1u : 0u);
            result.push_back(status.failure);
            result.push_back(status.droppedOutputMidi);
            result.push_back(status.mailboxOverruns);
            result.push_back(status.inputMidiTruncations);
            result.push_back(status.recoveryMode);
        }
        return result;
    }
    uint32_t get_last_sandbox_failure(uint32_t trackId) const {
        return m_engine ? m_engine->get_last_sandbox_failure(trackId) : 0u;
    }
    rust::String get_last_sandbox_failure_text(uint32_t trackId) const {
        return rust::String(m_engine ? m_engine->get_last_sandbox_failure_text(trackId)
                                     : "engine-unavailable");
    }
    rust::Vec<rust::String> get_sandbox_plugin_paths() const {
        rust::Vec<rust::String> result;
        if (!m_engine) return result;
        if (!m_engine) return result;
        for (const auto& path : m_engine->get_sandbox_plugin_paths())
            result.push_back(rust::String(path));
        return result;
    }
    rust::Vec<uint8_t> get_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex) const {
        rust::Vec<uint8_t> result;
        if (!m_engine) return result;
        for (const auto byte : m_engine->get_sandbox_plugin_state(trackId, sandboxIndex))
            result.push_back(byte);
        return result;
    }
    bool set_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex,
                                  rust::Slice<const uint8_t> state) const {
        if (!m_engine || state.size() > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes)
            return false;
        std::vector<uint8_t> bytes(state.begin(), state.end());
        return m_engine && m_engine->set_sandbox_plugin_state(trackId, sandboxIndex, bytes);
    }
    uint8_t get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine ? m_engine->get_sandbox_plugin_state_error(trackId, sandboxIndex) : 5u;
    }
    rust::String get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
        return rust::String(m_engine ? m_engine->get_sandbox_plugin_state_error_text(trackId, sandboxIndex)
                                     : "state-unavailable");
    }
    bool set_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                              uint32_t parameterId, float value) const {
        return m_engine && m_engine->set_plugin_parameter(tid, pluginIndex, parameterId, value);
    }
    bool set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId, float value) const {
        return m_engine && m_engine->set_plugin_parameter_without_undo(
            tid, pluginIndex, parameterId, value);
    }
    float get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                               uint32_t parameterId) const {
        return m_engine ? m_engine->get_plugin_parameter(tid, pluginIndex, parameterId) : 0.0f;
    }
    uint32_t get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
        return m_engine ? m_engine->get_plugin_parameter_count(tid, pluginIndex) : 0;
    }
    rust::String get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId) const {
        return rust::String(m_engine ? m_engine->get_plugin_parameter_name(tid, pluginIndex, parameterId) : "");
    }
    bool save_plugin_preset(uint32_t tid, uint32_t pluginIndex, rust::Str path) const {
        return m_engine && m_engine->save_plugin_preset(tid, pluginIndex, std::string(path));
    }
    bool load_plugin_preset(uint32_t tid, uint32_t pluginIndex, rust::Str path) const {
        return m_engine && m_engine->load_plugin_preset(tid, pluginIndex, std::string(path));
    }
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) const {
        return m_engine && m_engine->set_plugin_bypass(tid, pluginIndex, bypassed);
    }
    bool get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
        return m_engine && m_engine->get_plugin_bypass(tid, pluginIndex);
    }
    float get_track_latency_ms(uint32_t tid) const {
        return m_engine ? m_engine->get_track_latency_ms(tid) : 0.0f;
    }
    float get_track_pdc_compensation_ms(uint32_t tid) const {
        return m_engine ? m_engine->get_track_pdc_compensation_ms(tid) : 0.0f;
    }
    void set_macro_value(uint32_t macroIndex, float value) const {
        if (m_engine) m_engine->set_macro_value(macroIndex, value);
    }
    bool bind_midi_cc_to_macro(uint8_t channel, uint8_t cc, uint32_t macroIndex,
                               float minimum, float maximum, float curve,
                               bool pickup) const {
        return m_engine && m_engine->bind_midi_cc_to_macro(
            channel, cc, macroIndex, minimum, maximum, curve, pickup);
    }
    void handle_midi_cc(uint8_t channel, uint8_t cc, uint8_t value) const noexcept {
        if (m_engine) m_engine->handle_midi_cc(channel, cc, value);
    }
    void handle_midi_cc14(uint8_t channel, uint16_t controller, uint16_t value) const noexcept {
        if (m_engine) m_engine->handle_midi_cc14(channel, controller, value);
    }
    bool bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller, uint32_t macro_index,
                                 float minimum, float maximum, float curve,
                                 bool pickup) const noexcept {
        return m_engine && m_engine->bind_midi_cc14_to_macro(
            channel, controller, macro_index, minimum, maximum, curve, pickup);
    }
    void set_preview_synth_engine(uint32_t engine) const { if (m_engine) m_engine->set_preview_synth_engine(engine); }
    bool set_route(uint32_t sourceId, uint32_t destId, bool enabled) const {
        return m_engine && m_engine->set_route(sourceId, destId, enabled);
    }
    bool set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled) const {
        return m_engine && m_engine->set_route_gain(sourceId, destId, gain, enabled);
    }
    bool set_feedback_route(uint32_t sourceId, uint32_t destId, float gain, bool enabled) const {
        return m_engine && m_engine->set_feedback_route(sourceId, destId, gain, enabled);
    }
    bool set_sidechain_link(uint32_t sourceId, uint32_t destId,
                            uint32_t pluginIndex, uint32_t tapPoint, bool enabled) const {
        return m_engine && m_engine->set_sidechain_link(sourceId, destId, pluginIndex, tapPoint, enabled);
    }
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const {
        return m_engine && m_engine->has_sidechain_link(sourceId, destId, pluginIndex);
    }
    bool save_project(rust::Str path) const { return m_engine && m_engine->saveProject(std::string_view(path.data(), path.size())); }
    bool load_project(rust::Str path) const { return m_engine && m_engine->loadProject(std::string_view(path.data(), path.size())); }
    bool bounce_project(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project(std::string_view(path.data(), path.size()), format); }
    rust::String bounce_project_diagnostic_json(rust::Str path, uint32_t format) const {
        if (!m_engine) return rust::String("{\"code\":\"engine_unavailable\",\"retryable\":true}");
        const std::string_view output(path.data(), path.size());
        if (output.empty() || output.size() < 4 || output.substr(output.size() - 4) != ".wav") {
            return rust::String("{\"code\":\"invalid_render_path\",\"retryable\":false}");
        }
        if (format > 1) return rust::String("{\"code\":\"unsupported_render_format\",\"retryable\":false}");
        const bool ok = m_engine->bounce_project(std::string_view(path.data(), path.size()), format);
        return rust::String(ok ? "{\"ok\":true}" :
                            "{\"code\":\"bounce_failed\",\"retryable\":true}");
    }
    rust::String read_wav_diagnostic_json(rust::Str path, uint32_t format) const {
        const std::string input(path.data(), path.size());
        if (format > 1) {
            return rust::String("{\"ok\":false,\"code\":\"unsupported_wave_format\",\"retryable\":false}");
        }
        return rust::String(::Aura::IO::WavLoader::loadDiagnosticJson(input, format == 1));
    }
    bool bounce_project_async(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project_async(std::string_view(path.data(), path.size()), format); }
    float get_bounce_progress() const noexcept { return m_engine ? m_engine->get_bounce_progress() : 0.0f; }
    uint32_t get_bounce_state() const noexcept { return m_engine ? m_engine->get_bounce_state() : 0; }
    bool cancel_bounce() const noexcept { return m_engine && m_engine->cancel_bounce(); }
    uint32_t get_undo_count() const { return m_engine ? m_engine->get_undo_count() : 0; }
    uint32_t get_redo_count() const { return m_engine ? m_engine->get_redo_count() : 0; }
    void execute_restoration(uint32_t tid, float intensity) const { if (m_engine) m_engine->execute_restoration(tid, intensity); }
    bool execute_vocal_remover(uint32_t tid) const { return m_engine && m_engine->execute_vocal_remover(tid); }
    bool set_articulation_map(uint32_t tid, uint32_t mapHash) const { return m_engine && m_engine->set_articulation_map(tid, mapHash); }
    bool set_track_eq(uint32_t tid, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb) const {
        return m_engine && m_engine->set_track_eq(tid, lowBoostDb, lowCutDb, highBoostDb, highCutDb);
    }
    float get_track_correlation(uint32_t tid) const { return m_engine ? m_engine->get_track_correlation(tid) : 0.0f; }
    uint32_t get_track_count() const { return m_engine ? m_engine->get_track_count() : 0; }
    bool execute_auto_mixing() const { return m_engine && m_engine->execute_auto_mixing(); }
    bool execute_auto_arrangement() const { return m_engine && m_engine->execute_auto_arrangement(); }
    bool set_project_scale(int32_t root, int32_t type) const {
        return m_engine && m_engine->set_project_scale(root, type);
    }
    void undo() const { if (m_engine) m_engine->undo(); }
    void redo() const { if (m_engine) m_engine->redo(); }
    void begin_undo_transaction(rust::Str name) const {
        if (m_engine) m_engine->begin_undo_transaction(std::string_view(name.data(), name.size()));
    }
    bool end_undo_transaction() const { return m_engine && m_engine->end_undo_transaction(); }
    bool abort_undo_transaction() const { return m_engine && m_engine->abort_undo_transaction(); }

    bool move_region(uint32_t tid, uint32_t rid, double startBeat) const { return m_engine && m_engine->move_region(tid, rid, startBeat); }
    bool split_region(uint32_t tid, uint32_t rid, double split) const { return m_engine && m_engine->split_region(tid, rid, split); }
    bool remove_region(uint32_t tid, uint32_t rid) const { return m_engine && m_engine->remove_region(tid, rid); }
    uint32_t duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) const { return m_engine ? m_engine->duplicate_region(tid, rid, startSample) : 0; }
    bool set_region_gain(uint32_t tid, uint32_t rid, float gain) const { return m_engine && m_engine->set_region_gain(tid, rid, gain); }
    bool set_region_muted(uint32_t tid, uint32_t rid, bool muted) const { return m_engine && m_engine->set_region_muted(tid, rid, muted); }
    bool set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut) const { return m_engine && m_engine->set_region_fades(tid, rid, fadeIn, fadeOut); }
    bool set_region_reverse(uint32_t tid, uint32_t rid, bool reverse) const { return m_engine && m_engine->set_region_reverse(tid, rid, reverse); }
    bool set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio) const { return m_engine && m_engine->set_region_warp_ratio(tid, rid, ratio); }
    bool set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones) const { return m_engine && m_engine->set_region_pitch_semitones(tid, rid, semitones); }
    bool set_region_audio_note_segment(uint32_t tid, uint32_t rid, double startSeconds,
                                       double endSeconds, double pitchOffsetCents,
                                       double formantOffsetCents) const {
        return m_engine && m_engine->set_region_audio_note_segment(
            tid, rid, startSeconds, endSeconds, pitchOffsetCents, formantOffsetCents);
    }
    bool set_region_audio_note_anchor(uint32_t tid, uint32_t rid, double segmentStartSeconds,
                                      double positionSeconds, double pitchCents,
                                      double formantCents) const {
        return m_engine && m_engine->set_region_audio_note_anchor(
            tid, rid, segmentStartSeconds, positionSeconds, pitchCents, formantCents);
    }
    bool clear_region_audio_note_segments(uint32_t tid, uint32_t rid) const {
        return m_engine && m_engine->clear_region_audio_note_segments(tid, rid);
    }
    bool analyze_region_audio_note_segments(uint32_t tid, uint32_t rid, double sampleRate) const {
        return m_engine && m_engine->analyze_region_audio_note_segments(tid, rid, sampleRate);
    }
    bool set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count) const { return m_engine && m_engine->set_region_loop_count(tid, rid, count); }
    bool set_region_locked(uint32_t tid, uint32_t rid, bool locked) const { return m_engine && m_engine->set_region_locked(tid, rid, locked); }
    bool set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) const { return m_engine && m_engine->set_region_sync_group(tid, rid, group); }
    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, rust::Str operation, float parameter, bool enabled) const {
        return m_engine && m_engine->append_region_processing(tid, rid, stepId, std::string_view(operation.data(), operation.size()), parameter, enabled);
    }
    bool clear_region_processing(uint32_t tid, uint32_t rid) const { return m_engine && m_engine->clear_region_processing(tid, rid); }
    bool set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm) const { return m_engine && m_engine->set_region_trim(tid, rid, startNorm, endNorm); }
    void clear_midi_notes() const { if (m_engine) m_engine->clear_midi_notes(); }
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample) const { return m_engine && m_engine->remove_midi_notes_range(trackId, startSample, endSample); }
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones) const { return m_engine && m_engine->transpose_midi_notes_range(trackId, startSample, endSample, semitones); }
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples) const { return m_engine && m_engine->move_midi_notes_range(trackId, startSample, endSample, deltaSamples); }
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity, uint64_t startSample, uint64_t lengthSamples) const {
        if (m_engine) m_engine->set_midi_note(trackId, pitch, velocity, startSample, lengthSamples);
    }
    bool replace_midi_notes(rust::Vec<uint64_t> packed, bool recordUndo) const {
        if (!m_engine || packed.size() % 5 != 0 || packed.size() / 5 > 100000) return false;
        std::vector<::Aura::Core::Engine::AuraUnifiedEngine::ScheduledMidiNote> notes;
        notes.reserve(packed.size() / 5);
        for (size_t i = 0; i < packed.size(); i += 5) {
            const uint64_t track = packed[i];
            const uint64_t pitch = packed[i + 1];
            const uint64_t velocity = packed[i + 2];
            if (track > UINT32_MAX || pitch > 127 || velocity == 0 || velocity > 127 ||
                packed[i + 4] == 0 || packed[i + 3] > UINT64_MAX - packed[i + 4]) return false;
            notes.push_back({static_cast<uint32_t>(track), static_cast<uint8_t>(pitch),
                static_cast<uint8_t>(velocity), packed[i + 3], packed[i + 4]});
        }
        return m_engine->replace_midi_notes(notes, recordUndo);
    }
    rust::Vec<uint64_t> midi_notes_snapshot() const {
        rust::Vec<uint64_t> packed;
        if (!m_engine) return packed;
        const auto notes = m_engine->midi_notes_snapshot();
        packed.reserve(notes.size() * 5);
        for (const auto& note : notes) {
            packed.push_back(note.trackId);
            packed.push_back(note.pitch);
            packed.push_back(note.velocity);
            packed.push_back(note.startSample);
            packed.push_back(note.lengthSamples);
        }
        return packed;
    }
    bool set_spatial_position(uint32_t tid, float x, float y, float z) const { return m_engine && m_engine->set_spatial_position(tid, x, y, z); }
    void set_automation_record_mode(uint32_t mode) const {
        if (m_engine) m_engine->set_automation_record_mode(mode);
    }
    void set_automation_punch_range(uint64_t start, uint64_t end) const {
        if (m_engine) m_engine->set_automation_punch_range(start, end);
    }
    rust::String plugin_compatibility_snapshot_json() const { return m_engine ? rust::String(m_engine->plugin_compatibility_snapshot_json()) : rust::String("[]"); }
    void record_plugin_scan_failure(std::string_view id, std::string_view error) const { if (m_engine) m_engine->record_plugin_scan_failure(id, error); }
    void record_plugin_crash(std::string_view id) const { if (m_engine) m_engine->record_plugin_crash(id); }
    void set_plugin_blacklisted(std::string_view id, bool value) const { if (m_engine) m_engine->set_plugin_blacklisted(id, value); }
    bool set_automation_data(uint32_t tid, uint32_t paramId,
                             rust::Vec<double> packedPoints) const {
        std::vector<::Aura::Core::Engine::AuraUnifiedEngine::AutomationPoint> converted;
        if (packedPoints.size() % 3 != 0) return false;
        converted.reserve(packedPoints.size() / 3);
        double previousTime = -1.0;
        for (size_t i = 0; i < packedPoints.size(); i += 3) {
            const double time = packedPoints[i];
            const double value = packedPoints[i + 1];
            const double curve = packedPoints[i + 2];
            if (!std::isfinite(time) || !std::isfinite(value) || !std::isfinite(curve)
                || time < 0.0 || std::trunc(time) != time || time <= previousTime || value < 0.0 || value > 1.0
                || curve < -1.0 || curve > 1.0) return false;
            ::Aura::Core::Engine::AuraUnifiedEngine::AutomationPoint p;
            p.time = std::max(0.0, time);
            p.value = std::clamp(static_cast<float>(value), 0.0f, 1.0f);
            p.curve = std::clamp(static_cast<float>(curve), -1.0f, 1.0f);
            converted.push_back(p);
            previousTime = time;
        }
        return m_engine && m_engine->set_automation_data(tid, paramId, converted);
    }
    bool set_track_delay_automation(uint32_t tid, rust::Vec<double> packedPoints) const {
        std::vector<::Aura::Core::Engine::AuraUnifiedEngine::AutomationPoint> converted;
        if (packedPoints.size() % 3 != 0) return false;
        converted.reserve(packedPoints.size() / 3);
        double previousTime = -1.0;
        for (size_t i = 0; i < packedPoints.size(); i += 3) {
            const double time = packedPoints[i], value = packedPoints[i + 1], curve = packedPoints[i + 2];
            if (!std::isfinite(time) || !std::isfinite(value) || !std::isfinite(curve) ||
                time < 0.0 || std::trunc(time) != time || time <= previousTime ||
                value < 0.0 || value > 1.0 || curve < -1.0 || curve > 1.0) return false;
            converted.push_back({time, static_cast<float>(value), static_cast<float>(curve)});
            previousTime = time;
        }
        return m_engine && m_engine->set_track_delay_automation(tid, converted);
    }
    // Compatibility API: stop only this wrapper's device.  The underlying
    // AuraUnifiedEngine is a process-wide singleton and must not be shut down
    // when one Rust/FFI handle is dropped.
    void shutdown() const;

    // Explicit process-lifetime operation.  Call this only from the host's
    // final shutdown path, after all AudioEngine/AnalysisHub handles are gone.
    static void shutdown_process() noexcept;

    rust::Vec<float> get_engine_status_v() const;
    rust::Vec<float> get_runtime_health_v() const;
    bool audio_range_overflowed() const noexcept {
        return m_engine && m_engine->audio_range_overflowed();
    }
    rust::Vec<uint8_t> get_video_frame() const;
    uint64_t get_video_frame_revision() const noexcept {
        return m_engine ? m_engine->get_video_frame_revision() : 0;
    }
    bool request_video_frame(double seconds) const { return m_engine && m_engine->request_video_frame(seconds); }
    bool load_video(rust::Str path) const {
        return m_engine && m_engine->load_video(std::string_view(path.data(), path.size()));
    }
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_waveform(tid, rid, 128) : rust::Vec<float>{};
    }
    float get_cpu_total_v() const;
    rust::String get_project_layout_json() const;
    rust::String get_routing_snapshot_json() const;
    uint64_t get_project_generation() const {
        return m_engine ? m_engine->get_project_generation() : 0;
    }

    // Command Dispatch
    bool push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                      uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration) const {
        return m_engine && m_engine->push_command(type, tid, val, ts,
                                                   expectedProjectGeneration,
                                                   expectedAudioGeneration);
    }
    uint8_t get_last_command_error() const noexcept {
        return m_engine ? m_engine->get_last_command_error() : 1u;
    }
    
    // Core Access for telemetry shims
    const ::Aura::Core::Engine::AuraUnifiedEngine& get_core() const { return *m_engine; }
    std::shared_ptr<::Aura::Core::Engine::AuraUnifiedEngine> get_core_shared() const { return m_engine; }

private:
    // Session-owned graph. AnalysisHub receives a shared reference to this
    // same graph, while separate FFI handles receive separate project state.
    std::shared_ptr<::Aura::Core::Engine::AuraUnifiedEngine> m_engine;
    std::unique_ptr<AudioDriverHost> m_driver;
    mutable std::mutex m_configMutex;
};

} // namespace Aura::Core::BridgeFFI
