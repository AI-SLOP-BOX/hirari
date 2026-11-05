private:
    // Mirrors the format the device negotiated into the graph. Device selection
    // must apply the same transaction as any other reconfiguration, so the
    // quiesce/apply sequence lives here instead of once per driver branch.
    void applyNegotiatedDriverConfig() const {
        if (!m_driver || !m_engine) return;
        m_engine->waitForAudioCallbacks();
        ::Hirari::Core::Engine::EngineConfig cfg;
        cfg.tempo = hirari_tempo_map_get_current_bpm(
            reinterpret_cast<const void*>(m_engine->tempo_state()));
        cfg.sampleRate = hirari_audio_driver_host_sample_rate(m_driver);
        cfg.blockSize = hirari_audio_driver_host_buffer_size(m_driver);
        m_engine->apply_config(cfg);
    }
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
        if (inputs && inputChannelCount > 0 && inputs[0])
            self->m_engine->publish_input_monitor_block(inputs, inputChannelCount, frames);
        self->m_engine->publish_talkback_input_block(inputs, inputChannelCount, frames);
        std::array<float*, 32> mutableOutputs{};
        std::copy_n(outputs, outputChannelCount, mutableOutputs.begin());
        self->m_engine->processBlockDirect(mutableOutputs.data(), outputChannelCount, frames);
    }
    // Session-owned graph. Separate FFI handles receive separate project state.
    std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine> m_engine;
    // The Rust device host this session constructed.
    void* m_driver = nullptr;
    mutable std::mutex m_configMutex;
};
