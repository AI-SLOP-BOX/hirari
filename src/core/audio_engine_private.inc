private:
    struct ProjectLayoutSnapshotContext {
        explicit ProjectLayoutSnapshotContext(
            std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine> value)
            : engine(std::move(value)) {}
        std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine> engine;
        std::atomic<bool> cancelled{false};
    };
    struct ProjectLayoutSnapshotResult {
        uint64_t before = 0;
        uint64_t after = 0;
        std::string layout;
    };

    static bool computeProjectLayoutSnapshot(void* opaque, void** output) noexcept {
        auto* context = static_cast<ProjectLayoutSnapshotContext*>(opaque);
        if (!context || !context->engine || !output) return false;
        try {
            auto result = std::make_unique<ProjectLayoutSnapshotResult>();
            result->before = context->engine->get_project_state_revision();
            auto layout = context->engine->get_project_layout_json_cancellable(
                &context->cancelled);
            result->layout = layout.c_str();
            result->after = context->engine->get_project_state_revision();
            *output = result.release();
            return true;
        } catch (...) {
            return false;
        }
    }

    static bool readProjectLayoutSnapshot(
        void* opaque, uint64_t* before, uint64_t* after,
        const uint8_t** data, size_t* size) noexcept {
        const auto* result = static_cast<ProjectLayoutSnapshotResult*>(opaque);
        if (!result || !before || !after || !data || !size) return false;
        *before = result->before;
        *after = result->after;
        *data = reinterpret_cast<const uint8_t*>(result->layout.data());
        *size = result->layout.size();
        return true;
    }

    static void destroyProjectLayoutResult(void* opaque) noexcept {
        delete static_cast<ProjectLayoutSnapshotResult*>(opaque);
    }

    static void destroyProjectLayoutContext(void* opaque) noexcept {
        delete static_cast<ProjectLayoutSnapshotContext*>(opaque);
    }

    static void cancelProjectLayoutSnapshot(void* opaque) noexcept {
        if (auto* context = static_cast<ProjectLayoutSnapshotContext*>(opaque))
            context->cancelled.store(true, std::memory_order_release);
    }

    static bool isProjectLayoutSnapshotCancelled(const void* opaque) noexcept {
        const auto* context = static_cast<const ProjectLayoutSnapshotContext*>(opaque);
        return !context || context->cancelled.load(std::memory_order_acquire);
    }

    // Mirrors the format the device negotiated into the graph. Device selection
    // must apply the same transaction as any other reconfiguration, so the
    // quiesce/apply sequence lives here instead of once per driver branch.
    void applyNegotiatedDriverConfig() const {
#if defined(__APPLE__)
        if (!m_driver || !m_engine) return;
        m_engine->waitForAudioCallbacks();
        ::Hirari::Core::Engine::EngineConfig cfg;
        cfg.tempo = hirari_tempo_map_get_current_bpm(
            reinterpret_cast<const void*>(m_engine->tempo_state()));
        cfg.sampleRate = m_driver->sample_rate();
        cfg.blockSize = m_driver->buffer_size();
        m_engine->apply_config(cfg);
#endif
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
    uint64_t m_layoutSnapshotOwnerId = hirari_layout_snapshot_owner_create();
    std::unique_ptr<AudioDriverHost> m_driver;
    mutable std::mutex m_configMutex;
    mutable std::mutex m_inputPollMutex;
    mutable std::vector<float> m_inputPollScratch;
    // (track, plug-in) native editor sessions; the table lives in Rust.
    void* m_nativeEditorSessions = hirari_native_editor_sessions_create();
};
