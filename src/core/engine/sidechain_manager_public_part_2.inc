
    float getSidechainLevel(uint32_t destTrackId, uint32_t pluginIdx) const noexcept {
        if (pluginIdx >= kMaxPluginIndex) return 0.0f;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        for (const auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) == key)
                return slot.level.load(std::memory_order_relaxed);
        }
        return 0.0f;
    }

    bool controlRefreshSourceBuffer(uint32_t sourceTrackId, const float* left,
                                    const float* right, uint32_t frames,
                                    uint32_t sampleRate,
                                    uint64_t sourceGeneration) noexcept {
        if (!left || !right || frames == 0 || frames > kMaxBlockSize ||
            sampleRate == 0 || sourceGeneration == 0) return false;
        bool refreshed = false;
        ControlMutationGuard mutation(*this);
        for (auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) != kEmptyKey &&
                slot.sourceTrackId.load(std::memory_order_relaxed) == sourceTrackId) {
                if (slot.sourceGeneration.load(std::memory_order_relaxed) != sourceGeneration) continue;
                if (slot.sourceSampleRate.load(std::memory_order_relaxed) != sampleRate) continue;
                beginWrite(slot);
                const uint32_t copyFrames = frames;
                if (slot.owned.load(std::memory_order_relaxed) &&
                    copyFrames > 0 && copyFrames <= kMaxBlockSize) {
                    const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
                    const uint32_t next = 1u - active;
                    std::memcpy(slot.ownedLeft[next]->data(), left,
                                copyFrames * sizeof(float));
                    std::memcpy(slot.ownedRight[next]->data(), right,
                                copyFrames * sizeof(float));
                    std::fill(slot.ownedLeft[next]->begin() + copyFrames,
                              slot.ownedLeft[next]->end(), 0.0f);
                    std::fill(slot.ownedRight[next]->begin() + copyFrames,
                              slot.ownedRight[next]->end(), 0.0f);
                    slot.ownedFrames[next].store(copyFrames, std::memory_order_relaxed);
                    slot.ownedSampleRates[next].store(sampleRate, std::memory_order_relaxed);
                    slot.ownedSourceGenerations[next].store(sourceGeneration, std::memory_order_relaxed);
                    slot.ownedGenerations[next].store(
                        slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
                        std::memory_order_relaxed);
                    slot.sourceFrames.store(copyFrames, std::memory_order_relaxed);
                    slot.publishedBuffer.store(next, std::memory_order_release);
                    refreshed = true;
                }
                endWrite(slot);
            }
        }
        return refreshed;
    }

    // Audio-thread publication path. The slot's two owned buffers are
    // allocated by registerLink/registerSilentLink on the control thread.
    // Device reconfiguration and graph mutation quiesce callbacks before
    // changing the slot topology, so this path performs only bounded copies
    // into the inactive buffer and never takes the control writer mutex.
    void publishAudioSourceBlock(uint32_t sourceTrackId, const float* left,
                                 const float* right, uint32_t frames,
                                 uint32_t sampleRate,
                                 uint64_t sourceGeneration) noexcept {
        if (!left || !right || frames == 0 || frames > kMaxBlockSize ||
            sampleRate == 0 || sourceGeneration == 0) return;
        // Control publication and device publication are separate writers.
        // The audio path must never wait for the control mutex, so it uses an
        // admission flag plus a bounded CAS guard. A control mutation closes
        // admission before it reuses either owned buffer; a callback already
        // inside this section is allowed to finish and is waited on by the
        // control-side beginWrite().
        if (m_controlMutation.load(std::memory_order_acquire)) return;
        // A reader copies from the currently published buffer while holding
        // m_activeReaders.  One publication is safe when a reader arrives
        // after this check because the writer only touches the inactive
        // buffer.  A second publication before that reader leaves, however,
        // could rotate back and overwrite the buffer being copied.  Never
        // wait in the audio callback: skip this publication and let the
        // consumer retain the last coherent block instead.
        if (m_activeReaders.load(std::memory_order_acquire) != 0) return;
        bool expected = false;
        if (!m_audioPublishing.compare_exchange_strong(
                expected, true, std::memory_order_acq_rel,
                std::memory_order_relaxed)) {
            return;
        }
        if (m_controlMutation.load(std::memory_order_acquire)) {
            m_audioPublishing.store(false, std::memory_order_release);
            return;
        }
        // Re-check after admission so a reader that entered while the
        // publisher lock was acquired cannot be followed by a second buffer
        // rotation from this callback.
        if (m_activeReaders.load(std::memory_order_acquire) != 0) {
            m_audioPublishing.store(false, std::memory_order_release);
            return;
        }
        for (auto& slot : m_slots) {
            // Do not rotate another slot after a reader has entered.  This
            // keeps the two-buffer lifetime rule valid for every slot touched
            // by this publication pass without introducing an RT wait.
            if (m_activeReaders.load(std::memory_order_acquire) != 0) break;
            if (slot.key.load(std::memory_order_acquire) == kEmptyKey ||
                slot.sourceTrackId.load(std::memory_order_relaxed) != sourceTrackId ||
                !slot.owned.load(std::memory_order_relaxed)) continue;
            auto* leftBuffers = slot.ownedLeft[0].get();
            auto* rightBuffers = slot.ownedRight[0].get();
            if (!leftBuffers || !rightBuffers || !slot.ownedLeft[1] || !slot.ownedRight[1]) continue;

            // This writer is the single device callback publisher. The
            // control plane closes callback admission before modifying slots.
            slot.version.fetch_add(1, std::memory_order_acq_rel);
            const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
            const uint32_t next = 1u - active;
            std::memcpy(slot.ownedLeft[next]->data(), left, frames * sizeof(float));
            std::memcpy(slot.ownedRight[next]->data(), right, frames * sizeof(float));
            std::fill(slot.ownedLeft[next]->begin() + frames,
                      slot.ownedLeft[next]->end(), 0.0f);
            std::fill(slot.ownedRight[next]->begin() + frames,
                      slot.ownedRight[next]->end(), 0.0f);
            slot.ownedFrames[next].store(frames, std::memory_order_relaxed);
            slot.ownedSampleRates[next].store(sampleRate, std::memory_order_relaxed);
            slot.ownedSourceGenerations[next].store(sourceGeneration, std::memory_order_relaxed);
            slot.ownedGenerations[next].store(
                slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
                std::memory_order_relaxed);
            slot.sourceFrames.store(frames, std::memory_order_relaxed);
            slot.sourceSampleRate.store(sampleRate, std::memory_order_relaxed);
            slot.sourceGeneration.store(sourceGeneration, std::memory_order_relaxed);
            slot.publishedBuffer.store(next, std::memory_order_release);
            slot.left.store(slot.ownedLeft[next]->data(), std::memory_order_relaxed);
            slot.right.store(slot.ownedRight[next]->data(), std::memory_order_relaxed);
            slot.version.fetch_add(1, std::memory_order_release);
        }
        m_audioPublishing.store(false, std::memory_order_release);
    }

    [[deprecated("use controlRefreshSourceBuffer with mandatory generation metadata")]]
    bool refreshSourceBuffer(uint32_t sourceTrackId, const float* left,
                             const float* right, uint32_t frames,
                             uint32_t sampleRate, uint64_t sourceGeneration) noexcept {
        return controlRefreshSourceBuffer(sourceTrackId, left, right, frames,
                                          sampleRate, sourceGeneration);
    }

    /**
     * @brief RESOLVE: Connects sidechain destinations to source buffers with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating signal dependency tracking and tap-point resolution to the Rust 'RoutingOrchestrator'.
     */
    bool resolveSidechainLinks() noexcept {
        // This legacy entry point never carried enough information to publish
        // a safe, frame-bounded snapshot. Returning false is intentional: a
        // caller must use registerLink() instead of silently believing that a
        // sidechain was connected.
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    bool takeLegacyApiMisuse() noexcept {
        return m_legacyApiMisuse.exchange(false, std::memory_order_acq_rel);
    }


private:
