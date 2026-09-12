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
    void midi_clock_tick(uint64_t timestamp) const { if (m_engine) m_engine->midi_clock_tick(timestamp); }
    uint64_t midi_clock_ticks() const { return m_engine ? m_engine->midi_clock_ticks() : 0; }
    uint64_t midi_clock_last_tick() const { return m_engine ? m_engine->midi_clock_last_tick() : 0; }
    double midi_clock_rate() const { return m_engine ? m_engine->midi_clock_rate() : 120.0; }
    void set_midi_clock_rate(double bpm) const { if (m_engine) m_engine->set_midi_clock_rate(bpm); }
    bool set_track_volume(uint32_t tid, float value) const {
        return m_engine && m_engine->set_track_volume(tid, value);
    }
    float get_track_volume(uint32_t tid) const {
        return m_engine ? m_engine->get_track_volume(tid) : -1.0f;
    }
    bool set_master_gain(float value) const {
        return m_engine && m_engine->set_master_gain(value);
    }
    bool add_control_room_speaker(rust::Str name, float gain) const {
        return m_engine && m_engine->add_control_room_speaker(std::string(name), gain);
    }
    void reset_control_room() const { if (m_engine) m_engine->reset_control_room(); }
    bool select_control_room_speaker(uint32_t index) const { return m_engine && m_engine->select_control_room_speaker(index); }
    bool rename_control_room_speaker(uint32_t index, rust::Str name) const { return m_engine && m_engine->rename_control_room_speaker(index, std::string(name)); }
    bool remove_control_room_speaker(uint32_t index) const { return m_engine && m_engine->remove_control_room_speaker(index); }
    bool set_control_room_speaker_gain(uint32_t index, float gain) const { return m_engine && m_engine->set_control_room_speaker_gain(index, gain); }
    bool set_control_room_speaker_enabled(uint32_t index, bool enabled) const { return m_engine && m_engine->set_control_room_speaker_enabled(index, enabled); }
    bool upsert_control_room_cue(uint32_t id, float gain, bool enabled) const { return m_engine && m_engine->upsert_control_room_cue(id, gain, enabled); }
    bool remove_control_room_cue(uint32_t id) const { return m_engine && m_engine->remove_control_room_cue(id); }
    bool set_control_room_cue_enabled(uint32_t id, bool enabled) const { return m_engine && m_engine->set_control_room_cue_enabled(id, enabled); }
    float control_room_cue_gain(uint32_t id) const { return m_engine ? m_engine->control_room_cue_gain(id) : 0.0f; }
    bool control_room_validate() const { return m_engine && m_engine->control_room_validate(); }
    void set_control_room_dim(bool enabled) const { if (m_engine) m_engine->set_control_room_dim(enabled); }
    void set_control_room_talkback(bool enabled, float gain) const { if (m_engine) m_engine->set_control_room_talkback(enabled, gain); }
    bool control_room_dimmed() const { return m_engine && m_engine->control_room_dimmed(); }
    bool control_room_talkback_enabled() const { return m_engine && m_engine->control_room_talkback_enabled(); }
    float control_room_monitor_gain() const { return m_engine ? m_engine->control_room_monitor_gain() : 0.0f; }
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
    bool set_offline_render_range(uint64_t startSample, uint64_t endSample) const noexcept {
        return m_engine && m_engine->set_offline_render_range(startSample, endSample);
    }
    void clear_offline_render_range() const noexcept {
        if (m_engine) m_engine->clear_offline_render_range();
    }
    bool set_phase_invert(uint32_t tid, bool inverted) const {
        return m_engine && m_engine->set_phase_invert(tid, inverted);
    }
    bool set_track_armed(uint32_t tid, bool armed) const {
        return m_engine && m_engine->set_track_armed(tid, armed);
    }
    bool set_track_input_monitor(uint32_t tid, bool enabled) const {
        return m_engine && m_engine->set_track_input_monitor(tid, enabled);
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
#if defined(AURA_ENABLE_JACK)
        return rust::String(m_driver->status());
#endif
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
        if (!m_driver || !m_driver->select_device(deviceId, sampleRate, bufferSize) ||
            !m_driver->is_running() || !m_engine) return false;
        m_engine->waitForAudioCallbacks();
        ::Aura::Core::Engine::EngineConfig cfg;
        cfg.tempo = m_engine->get_tempo();
        cfg.sampleRate = m_driver->sample_rate();
        cfg.blockSize = m_driver->buffer_size();
        m_engine->apply_config(cfg);
        return std::abs(m_engine->get_sample_rate() - cfg.sampleRate) < 0.5 &&
               m_engine->get_block_size() == cfg.blockSize;
#elif defined(AURA_ENABLE_JACK)
        if (!m_driver || !m_driver->select_device(deviceId, sampleRate, bufferSize)) return false;
        if (!m_engine || !m_driver->is_running()) return false;
        m_engine->waitForAudioCallbacks();
        ::Aura::Core::Engine::EngineConfig cfg;
        cfg.tempo = m_engine->get_tempo();
        cfg.sampleRate = m_driver->sample_rate();
        cfg.blockSize = m_driver->buffer_size();
        m_engine->apply_config(cfg);
        return std::abs(m_engine->get_sample_rate() - cfg.sampleRate) < 0.5 &&
               m_engine->get_block_size() == cfg.blockSize;
#else
        (void)deviceId; (void)sampleRate; (void)bufferSize; return false;
#endif
    }
    uint64_t get_audio_config_generation() const {
        return m_engine ? m_engine->get_audio_config_generation() : 0;
    }
    uint32_t get_block_size() const { return m_engine ? m_engine->get_block_size() : 0; }
    float get_latency_ms() const { return m_engine ? m_engine->get_latency_ms() : 0.0f; }
    float get_master_tail_ms() const { return m_engine ? m_engine->get_master_tail_ms() : 0.0f; }
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
#if !defined(__APPLE__) && !defined(AURA_ENABLE_JACK)
        // Keep the non-hardware fallback on the same callback boundary as a
        // real driver. The dispatcher invokes process_driver_block, which
        // owns the single native graph render; do not render it a second time
        // below.
        if (m_driver && m_driver->dispatch_process_callback(
                left.data(), right.data(), static_cast<uint32_t>(left.size()))) return;
#endif
        float* channels[2] = {left.data(), right.data()};
        if (m_engine) m_engine->processBlockDirect(channels, 2, static_cast<uint32_t>(left.size()));
    }

    bool quantize_audio_group_stereo(rust::Slice<float> left, rust::Slice<float> right,
                                     float bpm, double sample_rate, float strength,
                                     float swing) const noexcept {
        if (!m_engine || left.size() == 0 || left.size() != right.size()) return false;
        float* buffers[2] = {left.data(), right.data()};
        return m_engine->quantizeAudioGroup(buffers, 2, left.size(), bpm, sample_rate,
                                            strength, swing);
    }
    // Phase-coherent group warp for an arbitrary number of linked microphones.
    // `planar` is laid out channel-by-channel (all frames of ch0, then ch1...).
    bool quantize_audio_group(rust::Slice<float> planar, uint32_t channels,
                              uint64_t frames, float bpm, double sample_rate,
                              float strength, float swing) const noexcept {
        if (!m_engine || channels == 0 || channels > 32 || frames == 0 ||
            frames > ::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize ||
            planar.size() != static_cast<size_t>(channels) * frames) return false;
        std::array<float*, 32> buffers{};
        for (uint32_t c = 0; c < channels; ++c) buffers[c] = planar.data() + static_cast<size_t>(c) * frames;
        return m_engine->quantizeAudioGroup(buffers.data(), channels, frames, bpm,
                                            sample_rate, strength, swing);
    }

    void process_pitch_shift_block(rust::Slice<float> left, rust::Slice<float> right,
                                   float ratio, float fundamental_hz,
                                   double sample_rate, float formant_ratio,
                                   float timing_ratio) const noexcept {
        if (left.empty() || left.size() != right.size() || left.size() > ::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize ||
            !std::isfinite(ratio) || ratio < 0.5f || ratio > 2.0f || !std::isfinite(fundamental_hz) || fundamental_hz <= 0.0f ||
            !std::isfinite(sample_rate) || sample_rate < 8'000.0 || sample_rate > 384'000.0 ||
            !std::isfinite(formant_ratio) || formant_ratio < 0.25f || formant_ratio > 4.0f ||
            !std::isfinite(timing_ratio) || timing_ratio < 0.25f || timing_ratio > 4.0f) return;
        static thread_local ::Aura::Core::DSP::Vocal::PsolaPitchShifter shifterL;
        static thread_local ::Aura::Core::DSP::Vocal::PsolaPitchShifter shifterR;
        static thread_local double lastSampleRate = 0.0;
        if (lastSampleRate != sample_rate) {
            shifterL.reset();
            shifterR.reset();
            lastSampleRate = sample_rate;
        }
        static thread_local std::vector<float> inL;
        static thread_local std::vector<float> inR;
        if (inL.capacity() < left.size()) inL.reserve(::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize);
        if (inR.capacity() < right.size()) inR.reserve(::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize);
        inL.assign(left.data(), left.data() + left.size());
        inR.assign(right.data(), right.data() + right.size());
        shifterL.process(inL.data(), left.data(), static_cast<uint32_t>(left.size()), ratio, fundamental_hz, sample_rate, formant_ratio, timing_ratio);
        shifterR.process(inR.data(), right.data(), static_cast<uint32_t>(right.size()), ratio, fundamental_hz, sample_rate, formant_ratio, timing_ratio);
    }

    // Offline spectral-canvas edit exposed at the same bridge boundary as
    // pitch processing. The AudioBuffer wraps caller-owned slices, so no
    // second copy of the clip is made; the processor validates all bounds and
    // keeps edits out of the realtime callback path.
    bool apply_spectral_gain_stereo(rust::Slice<float> left, rust::Slice<float> right,
                                    float t0, float f0, float t1, float f1,
                                    float gain, double sample_rate) const noexcept {
        if (left.empty() || left.size() != right.size() ||
            left.size() > 16u * 1024u * 1024u ||
            !std::isfinite(t0) || !std::isfinite(f0) || !std::isfinite(t1) ||
            !std::isfinite(f1) || !std::isfinite(gain) ||
            !std::isfinite(sample_rate) || sample_rate < 8'000.0 ||
            sample_rate > 384'000.0) return false;
        float* channels[2] = {left.data(), right.data()};
        ::Aura::Core::AudioBuffer view;
        view.wrapChannels(channels, 2, static_cast<uint32_t>(left.size()));
        ::Aura::DSP::Analysis::SpectralProcessor processor;
        processor.applySpectralGain(view, sample_rate,
                                    {t0, f0, t1, f1}, gain);
        for (size_t i = 0; i < left.size(); ++i)
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        return true;
    }

    bool reduce_noise_stereo(rust::Slice<float> left, rust::Slice<float> right,
                             float amount, float profile_seconds,
                             double sample_rate) const noexcept {
        if (left.empty() || left.size() != right.size() ||
            left.size() > 16u * 1024u * 1024u || !std::isfinite(amount) ||
            !std::isfinite(profile_seconds) || !std::isfinite(sample_rate) ||
            sample_rate < 8'000.0 || sample_rate > 384'000.0) return false;
        float* channels[2] = {left.data(), right.data()};
        ::Aura::Core::AudioBuffer view;
        view.wrapChannels(channels, 2, static_cast<uint32_t>(left.size()));
        ::Aura::DSP::Analysis::SpectralProcessor processor;
        processor.reduceNoise(view, sample_rate, amount, profile_seconds);
        for (size_t i = 0; i < left.size(); ++i)
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        return true;
    }

    bool repair_clipped_stereo(rust::Slice<float> left, rust::Slice<float> right,
                               float ceiling) const noexcept {
        if (left.empty() || left.size() != right.size() ||
            left.size() > 16u * 1024u * 1024u || !std::isfinite(ceiling)) return false;
        float* channels[2] = {left.data(), right.data()};
        ::Aura::Core::AudioBuffer view;
        view.wrapChannels(channels, 2, static_cast<uint32_t>(left.size()));
        ::Aura::DSP::Analysis::SpectralProcessor processor;
        (void)processor.repairClipped(view, ceiling);
        for (size_t i = 0; i < left.size(); ++i)
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        return true;
    }

    bool remove_hum_stereo(rust::Slice<float> left, rust::Slice<float> right,
                           float fundamental_hz, uint32_t harmonics,
                           float bandwidth_hz, float t0, float f0,
                           float t1, float f1, double sample_rate) const noexcept {
        if (left.empty() || left.size() != right.size() ||
            left.size() > 16u * 1024u * 1024u || !std::isfinite(fundamental_hz) ||
            fundamental_hz <= 0.0f || harmonics == 0 ||
            !std::isfinite(bandwidth_hz) || bandwidth_hz <= 0.0f ||
            !std::isfinite(t0) || !std::isfinite(f0) || !std::isfinite(t1) ||
            !std::isfinite(f1) || !std::isfinite(sample_rate) ||
            sample_rate < 8'000.0 || sample_rate > 384'000.0) return false;
        float* channels[2] = {left.data(), right.data()};
        ::Aura::Core::AudioBuffer view;
        view.wrapChannels(channels, 2, static_cast<uint32_t>(left.size()));
        ::Aura::DSP::Analysis::SpectralProcessor processor;
        processor.removeHum(view, sample_rate, fundamental_hz,
                             {t0, f0, t1, f1}, harmonics, bandwidth_hz);
        for (size_t i = 0; i < left.size(); ++i)
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        return true;
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
#if defined(AURA_ENABLE_JACK)
        if (m_driver && m_driver->is_running() &&
            !m_driver->reconfigure(static_cast<double>(sr), bs)) {
            return false;
        }
#endif
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

