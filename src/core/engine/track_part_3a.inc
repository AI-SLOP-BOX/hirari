    // The channel EQ write moved to Rust (engine_analysis_reads.rs), which maps
    // the dB pairs through the shared gain policy and writes the runtime.
    // The Rust runtime retunes the region store, the strip and the panner,
    // grows every block the track renders through and prepares the plug-in
    // chain. Only the allocation policy stays here because it is a C++
    // exception, so the status only has to name it.
    virtual void prepareToPlay(double sr, uint32_t blockSize) {
        const uint8_t status = hirari_track_runtime_prepare(m_runtime, this, sr, blockSize);
        if (status == 2) throw std::bad_array_new_length();
        if (status == 3) throw std::bad_alloc();
    }
    void resetForOfflineRender() noexcept {
        hirari_track_runtime_reset_for_offline_render(m_runtime);
    }

    // Live edits invalidate the rendered snapshot so stale audio cannot
    // survive a parameter, region, or automation mutation. The audio thread
    // only observes the raw pointer the Rust freeze host published, and the
    // store keeps the replaced render alive until the control side retires it
    // after quiescing, so dropping one here cannot invalidate a live callback.
    void invalidateFrozenForEdit() noexcept {
        hirari_track_frozen_store_clear(m_frozenStore);
        markProjectLayoutChanged();
    }

    // Offline render taps are prepared on the control thread and only copied
    // on the audio side. The runtime addresses them through the handles bound
    // at construction, so no C++ accessor exposes the storage type.
    void setSpatialPosition(float x, float y, float z) {
        // Spatial coordinates are control-thread input. Never publish a
        // non-finite value to the real-time panner.
        const auto sanitize = [](float value) {
            return std::isfinite(value) ? std::clamp(value, -100.0f, 100.0f) : 0.0f;
        };
        editControl([&] {
            hirari_track_runtime_set_spatial(m_runtime, sanitize(x), sanitize(y), sanitize(z));
        });
    }
    bool setHrtfKernel(const float* left, const float* right, uint32_t taps) {
        if (!left || !right || taps == 0 || taps > kMaxHrtfTaps) return false;
        invalidateFrozenForEdit();
        const bool changed =
            hirari_track_holographic_panner_set_kernel(m_spatialPanner, left, right, taps);
        if (changed) markProjectLayoutChanged();
        return changed;
    }
    void clearHrtfKernel() {
        editControl(
            [&] { hirari_track_holographic_panner_clear_kernel(m_spatialPanner); });
    }
    bool isMuted() const { return hirari_track_runtime_is_muted(m_runtime); }
    void setMuted(bool muted) {
        editControl([&] { hirari_track_runtime_set_muted(m_runtime, static_cast<uint8_t>(muted)); });
    }
    bool isPhaseInverted() const { return hirari_track_runtime_is_phase_inverted(m_runtime); }
    void setPhaseInverted(bool inverted) {
        editControl([&] {
            hirari_track_runtime_set_phase_invert(m_runtime, static_cast<uint8_t>(inverted));
        });
    }
    void setSolo(bool solo) {
        editControl([&] { hirari_channel_strip_set_solo(m_strip, solo); });
    }
    bool isSolo() const { return hirari_channel_strip_is_solo(m_strip); }
    // The Rust region store owns the table; the runtime publishes its render
    // inputs to the audio callback.
    void installRegionInputs(const HirariTrackRegionRenderInput* inputs,
                             size_t length, uint8_t published) noexcept {
        hirari_track_runtime_publish_regions(
            m_runtime, reinterpret_cast<const uint8_t*>(inputs), length, published);
    }
    static uint32_t regionHostAudioView(void* handle, const float** out,
                                        size_t capacity, uint64_t* samples) {
        if (handle == nullptr) return 0;
        const auto storage = *static_cast<const SharedAudioStorage*>(handle);
        void* buffer = storage.get();
        if (!buffer) return 0;
        const uint32_t channels = hirari_audio_buffer_storage_channels(buffer);
        if (samples) {
            *samples = static_cast<uint64_t>(hirari_audio_buffer_storage_samples(buffer));
        }
        if (out && capacity >= channels) {
            for (uint32_t channel = 0; channel < channels; ++channel) {
                out[channel] = hirari_audio_buffer_storage_read_pointer(buffer, channel, 0);
            }
        }
        return channels;
    }
    static void* regionHostRetainHandle(void* handle) {
        return new SharedAudioStorage(
            *static_cast<SharedAudioStorage*>(handle));
    }
    static void regionHostReleaseHandle(void* handle) {
        delete static_cast<SharedAudioStorage*>(handle);
    }
    static void regionHostInstall(void* track, const HirariTrackRegionRenderInput* inputs,
                                  size_t length, uint8_t published) {
        static_cast<Track*>(track)->installRegionInputs(inputs, length, published);
    }
    static void regionHostInvalidateFrozen(void* track) {
        static_cast<Track*>(track)->invalidateFrozenForEdit();
    }
    static void regionHostMarkLayout(void* track) {
        static_cast<Track*>(track)->markProjectLayoutChanged();
    }
    static bool regionHostPitchRunning(void* track) {
        return static_cast<Track*>(track)->audioPitchAnalysisRunning();
    }
    static constexpr HirariRegionStorageHostOps kRegionStorageHost = {
        &Track::regionHostAudioView, &Track::regionHostRetainHandle,
        &Track::regionHostReleaseHandle, &Track::regionHostInstall,
        &Track::regionHostInvalidateFrozen, &Track::regionHostMarkLayout,
        &Track::regionHostPitchRunning};
    bool beginAudioPitchAnalysis() noexcept {
        return hirari_channel_strip_begin_pitch_analysis(m_strip);
    }
    void finishAudioPitchAnalysis() noexcept {
        hirari_channel_strip_finish_pitch_analysis(m_strip);
    }
    bool audioPitchAnalysisRunning() const noexcept {
        return hirari_channel_strip_is_pitch_analysis_running(m_strip);
    }

    // Audio-thread producer for write automation. The queue is deliberately
    // bounded and SPSC: recording a control surface gesture must never grow a
    // vector, take an edit lock, or allocate inside processBlock().
