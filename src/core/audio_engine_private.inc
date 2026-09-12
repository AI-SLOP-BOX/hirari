private:
#if !defined(__APPLE__)
    void sync_jack_graph_config() const {
#if defined(AURA_ENABLE_JACK)
        if (!m_driver || !m_driver->is_running() || !m_engine) return;
        m_engine->waitForAudioCallbacks();
        ::Aura::Core::Engine::EngineConfig cfg;
        cfg.tempo = m_engine->get_tempo();
        cfg.sampleRate = m_driver->sample_rate();
        cfg.blockSize = m_driver->buffer_size();
        m_engine->apply_config(cfg);
#endif
    }
#endif
    static void process_driver_block(const float* const* inputs, float* const* outputs,
                                     uint32_t frames, void* context) noexcept {
        auto* self = static_cast<AudioEngine*>(context);
        if (!self || !self->m_engine || !outputs || !outputs[0] || !outputs[1] || frames == 0 ||
            frames > ::Aura::Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize) return;
#if defined(__APPLE__) || defined(AURA_ENABLE_JACK)
        if (self->m_driver && inputs && inputs[0] && inputs[1])
            self->m_driver->capture_input(inputs, 2, frames);
#endif
        if (inputs && inputs[0] && inputs[1])
            self->m_engine->publish_input_monitor_block(inputs, 2, frames);
        float* channels[2] = {outputs[0], outputs[1]};
        self->m_engine->processBlockDirect(channels, 2, frames);
#if defined(AURA_ENABLE_JACK)
        const float peak = std::max(std::abs(*std::max_element(outputs[0], outputs[0] + frames,
            [](float a, float b) { return std::abs(a) < std::abs(b); })),
            std::abs(*std::max_element(outputs[1], outputs[1] + frames,
            [](float a, float b) { return std::abs(a) < std::abs(b); })));
        if (self->m_driver) self->m_driver->record_callback(frames, std::isfinite(peak) ? peak : 0.0f);
#endif
    }
    // Session-owned graph. AnalysisHub receives a shared reference to this
    // same graph, while separate FFI handles receive separate project state.
    std::shared_ptr<::Aura::Core::Engine::AuraUnifiedEngine> m_engine;
    inline static std::atomic<uint64_t> s_waveformSequence{0};
    inline static std::mutex s_waveformMutex;
    inline static std::unordered_set<uint64_t> s_waveformPending;
    inline static std::unordered_map<uint64_t, std::vector<float>> s_waveformResults;
    std::unique_ptr<AudioDriverHost> m_driver;
    mutable std::mutex m_configMutex;
    mutable ::Aura::Core::Plugins::NativeEditorHost m_nativeEditorHost;
};
