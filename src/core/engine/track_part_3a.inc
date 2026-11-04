    void setEq(float lowBoostDb, float lowCutDb, float highBoostDb, float highCutDb) {
        hirari_track_runtime_set_eq(
            m_runtime, hirari_track_eq_gain(lowBoostDb, lowCutDb),
            hirari_track_eq_gain(highBoostDb, highCutDb));
    }
    virtual void prepareToPlay(double sr, uint32_t blockSize) {
        if (!std::isfinite(sr) || sr <= 0.0 || blockSize == 0) return;
        m_sampleRate = sr;
        (void)hirari_region_store_prepare(this, sr, blockSize);
        hirari_channel_strip_set_sample_rate(m_strip, sr);
        hirari_track_holographic_panner_set_sample_rate(m_spatialPanner, sr);
        // Preparing a track is also the boundary used by offline renders and
        // device reinitialisation. Snap smoothed channel-strip state to its
        // current targets so the first block is deterministic after reload.
        hirari_channel_strip_reset(m_strip);
        m_activeBuffer.resize(2, blockSize);
        m_preFaderBuffer.resize(2, blockSize);
        m_sendPreFaderBuffer.resize(2, blockSize);
        m_sendPostFaderBuffer.resize(2, blockSize);
        m_preInsertBuffer.resize(2, blockSize);
        (void)hirari_effect_chain_set_sample_rate(m_chain, sr, blockSize);
    }
    void resetForOfflineRender() noexcept {
        hirari_track_runtime_reset_for_offline_render(m_runtime);
    }

    // Publishes a fully rendered, immutable track snapshot.  Loading and
    // decoding are control-thread responsibilities; the audio thread only
    // observes the raw pointer and copies bounded samples from it.  Published
    // buffers are retained for the lifetime of the Track so replacing a
    // freeze cannot invalidate a callback already in flight.
    // The freeze store retains the owner handles, so a replaced buffer stays
    // alive until the control side explicitly retires it after quiescing.
    bool publishFrozenAudio(std::shared_ptr<AudioBuffer> rendered,
                            uint64_t totalSamples,
                            uint32_t sampleRate,
                            uint64_t projectGeneration = 0,
                            uint64_t audioGeneration = 0,
                            std::string_view cachePath = {}) {
        if (!rendered) return false;
        auto* owner = new std::shared_ptr<AudioBuffer>(std::move(rendered));
        if (!hirari_track_frozen_store_publish(
                m_frozenStore, owner, (*owner)->storageHandle(), (*owner)->getNumChannels(),
                (*owner)->getNumSamples(), totalSamples, sampleRate,
                projectGeneration, audioGeneration,
                reinterpret_cast<const uint8_t*>(cachePath.data()), cachePath.size())) {
            delete owner;
            return false;
        }
        markProjectLayoutChanged();
        return true;
    }

    void clearFrozenAudio() noexcept {
        hirari_track_frozen_store_clear(m_frozenStore);
        markProjectLayoutChanged();
    }

    // Control side only, after the audio callback has been quiesced.
    void collectRetiredFrozenAudio() { hirari_track_frozen_store_retire(m_frozenStore); }

    bool isFrozen() const noexcept { return hirari_track_frozen_store_is_frozen(m_frozenStore); }
    // Live edits invalidate the rendered snapshot so stale audio cannot
    // survive a parameter, region, or automation mutation.
    void invalidateFrozenForEdit() noexcept {
        clearFrozenAudio();
    }
    uint64_t frozenTotalSamples() const noexcept {
        return hirari_track_frozen_store_total_samples(m_frozenStore);
    }
    uint32_t frozenSampleRate() const noexcept {
        return hirari_track_frozen_store_sample_rate(m_frozenStore);
    }
    uint64_t frozenProjectGeneration() const noexcept {
        return hirari_track_frozen_store_project_generation(m_frozenStore);
    }
    uint64_t frozenAudioGeneration() const noexcept {
        return hirari_track_frozen_store_audio_generation(m_frozenStore);
    }
    std::string frozenPath() const {
        std::string path(hirari_track_frozen_store_path_len(m_frozenStore), '\0');
        (void)hirari_track_frozen_store_path_copy(
            m_frozenStore, reinterpret_cast<uint8_t*>(path.data()), path.size());
        return path;
    }
    bool setFrozenCachePath(std::string_view path) {
        if (!hirari_track_frozen_store_set_path(
                m_frozenStore, reinterpret_cast<const uint8_t*>(path.data()), path.size())) {
            return false;
        }
        markProjectLayoutChanged();
        return true;
    }

    // Offline render taps are prepared on the control thread and only copied
    // on the audio side. They let the render graph select a real pre-fader or
    // pre-insert signal without changing the interactive mix state.
    const AudioBuffer& offlinePreFaderBuffer() const noexcept { return m_preFaderBuffer; }
    const AudioBuffer& sendPreFaderBuffer() const noexcept { return m_sendPreFaderBuffer; }
    const AudioBuffer& sendPostFaderBuffer() const noexcept { return m_sendPostFaderBuffer; }
    const AudioBuffer& offlinePreInsertBuffer() const noexcept { return m_preInsertBuffer; }
    const AudioBuffer& preFxSidechainBuffer() const noexcept { return m_preInsertBuffer; }
    const AudioBuffer& postFxSidechainBuffer() const noexcept { return m_preFaderBuffer; }
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
    float getSpatialX() const { return hirari_track_runtime_spatial(m_runtime, 0); }
    float getSpatialY() const { return hirari_track_runtime_spatial(m_runtime, 1); }
    float getSpatialZ() const { return hirari_track_runtime_spatial(m_runtime, 2); }
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
        editControl([&] { m_solo.store(solo, std::memory_order_relaxed); });
    }
    bool isSolo() const { return m_solo.load(std::memory_order_relaxed); }
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
        const auto audio = *static_cast<const std::shared_ptr<AudioBuffer>*>(handle);
        if (!audio) return 0;
        const uint32_t channels = static_cast<uint32_t>(audio->getNumChannels());
        if (samples) *samples = static_cast<uint64_t>(audio->getNumSamples());
        if (out && capacity >= channels) {
            for (uint32_t channel = 0; channel < channels; ++channel) {
                out[channel] = audio->getReadPointer(channel);
            }
        }
        return channels;
    }
    static void* regionHostRetainHandle(void* handle) {
        return new std::shared_ptr<AudioBuffer>(
            *static_cast<std::shared_ptr<AudioBuffer>*>(handle));
    }
    static void regionHostReleaseHandle(void* handle) {
        delete static_cast<std::shared_ptr<AudioBuffer>*>(handle);
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
        bool expected = false;
        return m_audioPitchAnalysisRunning.compare_exchange_strong(
            expected, true, std::memory_order_acq_rel);
    }
    void finishAudioPitchAnalysis() noexcept {
        m_audioPitchAnalysisRunning.store(false, std::memory_order_release);
    }
    bool audioPitchAnalysisRunning() const noexcept {
        return m_audioPitchAnalysisRunning.load(std::memory_order_acquire);
    }

    // Audio-thread producer for write automation. The queue is deliberately
    // bounded and SPSC: recording a control surface gesture must never grow a
    // vector, take an edit lock, or allocate inside processBlock().
