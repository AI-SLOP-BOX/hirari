private:
#if !defined(__APPLE__)
    void sync_jack_graph_config() const {
#if defined(HIRARI_ENABLE_JACK) && !defined(__APPLE__)
        if (!m_driver || !m_driver->is_running() || !m_engine) return;
        m_engine->waitForAudioCallbacks();
        ::Hirari::Core::Engine::EngineConfig cfg;
        cfg.tempo = m_engine->get_tempo();
        cfg.sampleRate = m_driver->sample_rate();
        cfg.blockSize = m_driver->buffer_size();
        m_engine->apply_config(cfg);
#endif
    }
#endif
    static void process_driver_block(const float* const* inputs, uint32_t inputChannelCount,
                                     float* const* outputs, uint32_t outputChannelCount,
                                     uint32_t frames,
                                     void* context) noexcept {
        auto* self = static_cast<AudioEngine*>(context);
        if (!self || !self->m_engine || !outputs || outputChannelCount < 2 ||
            outputChannelCount > 32 || frames == 0 ||
            frames > ::Hirari::Core::Engine::HirariUnifiedEngine::kMaxAudioBlockSize) return;
        for (uint32_t channel = 0; channel < outputChannelCount; ++channel)
            if (!outputs[channel]) return;
#if defined(HIRARI_ENABLE_JACK) && !defined(__APPLE__)
        if (self->m_driver && inputs && inputChannelCount > 0 && inputs[0])
            self->m_driver->capture_input(inputs, inputChannelCount, frames);
#endif
        if (inputs && inputChannelCount > 0 && inputs[0])
            self->m_engine->publish_input_monitor_block(inputs, inputChannelCount, frames);
        self->m_engine->publish_talkback_input_block(inputs, inputChannelCount, frames);
        std::array<float*, 32> mutableOutputs{};
        std::copy_n(outputs, outputChannelCount, mutableOutputs.begin());
        self->m_engine->processBlockDirect(mutableOutputs.data(), outputChannelCount, frames);
#if defined(HIRARI_ENABLE_JACK)
        float peak = 0.0f;
        for (uint32_t channel = 0; channel < outputChannelCount; ++channel)
            for (uint32_t i = 0; i < frames; ++i)
                peak = std::max(peak, std::abs(outputs[channel][i]));
        if (self->m_driver) self->m_driver->record_callback(frames, std::isfinite(peak) ? peak : 0.0f);
#endif
    }
    // Session-owned graph. AnalysisHub receives a shared reference to this
    // same graph, while separate FFI handles receive separate project state.
    std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine> m_engine;
    inline static std::atomic<uint64_t> s_waveformSequence{0};
    inline static std::mutex s_waveformMutex;
    inline static std::unordered_set<uint64_t> s_waveformPending;
    inline static std::unordered_map<uint64_t, std::vector<float>> s_waveformResults;
    std::unique_ptr<AudioDriverHost> m_driver;
    mutable std::mutex m_configMutex;
    mutable std::mutex m_inputPollMutex;
    mutable std::vector<float> m_inputPollScratch;
    mutable ::Hirari::Core::Plugins::NativeEditorHost m_nativeEditorHost;
    struct ProjectLayoutSnapshotState {
        std::mutex mutex;
        uint64_t nextId = 1;
        uint64_t activeId = 0;
        uint64_t resultId = 0;
        bool hasResult = false;
        std::string result;
        std::shared_ptr<std::atomic<bool>> activeCancellation;
    };
    std::shared_ptr<ProjectLayoutSnapshotState> m_projectLayoutSnapshotState =
        std::make_shared<ProjectLayoutSnapshotState>();
};
