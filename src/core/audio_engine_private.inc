private:
    // The negotiated device format is applied by Rust (engine_config.rs), which
    // reaches the driver host, the tempo map and the input-block store through
    // their own externs, so this facade keeps no config-transaction member.
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
        // The caller owns these buffers; the Rust input store copies what the
        // graph needs so no driver pointer is retained beyond the callback.
        if (inputs && inputChannelCount > 0 && inputs[0])
            hirari_input_blocks_publish_monitor(self->m_engine->input_blocks(), inputs,
                                                 inputChannelCount, frames);
        hirari_input_blocks_publish_talkback(self->m_engine->input_blocks(), inputs,
                                             inputChannelCount, frames,
                                             self->m_engine->control_room_state());
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
