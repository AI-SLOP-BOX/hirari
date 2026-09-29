    float getSidechainLevel(uint32_t destination, uint32_t plugin) const noexcept {
        return hirari_sidechain_level(m_publicationGate, destination, plugin);
    }

    bool controlRefreshSourceBuffer(uint32_t source, const float* left,
                                    const float* right, uint32_t frames,
                                    uint32_t sampleRate,
                                    uint64_t sourceGeneration) noexcept {
        ControlMutationGuard mutation(*this);
        return hirari_sidechain_refresh_source(
            m_publicationGate, source, left, right, frames, sampleRate, sourceGeneration);
    }

    void publishAudioSourceBlock(uint32_t source,
                                 const float* preLeft, const float* preRight,
                                 const float* postLeft, const float* postRight,
                                 const float* faderLeft, const float* faderRight,
                                 uint32_t frames, uint32_t sampleRate,
                                 uint64_t sourceGeneration) noexcept {
        if (!hirari_sidechain_publication_gate_try_begin_audio(m_publicationGate)) return;
        hirari_sidechain_publish_source(
            m_publicationGate, source, preLeft, preRight, postLeft, postRight,
            faderLeft, faderRight, frames, sampleRate, sourceGeneration);
        hirari_sidechain_publication_gate_end_audio(m_publicationGate);
    }

    [[deprecated("use controlRefreshSourceBuffer with mandatory generation metadata")]]
    bool refreshSourceBuffer(uint32_t source, const float* left, const float* right,
                             uint32_t frames, uint32_t sampleRate,
                             uint64_t sourceGeneration) noexcept {
        return controlRefreshSourceBuffer(
            source, left, right, frames, sampleRate, sourceGeneration);
    }

    bool resolveSidechainLinks() noexcept {
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    bool takeLegacyApiMisuse() noexcept {
        return m_legacyApiMisuse.exchange(false, std::memory_order_acq_rel);
    }
