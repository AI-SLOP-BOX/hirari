        // The whole post-graph tail is Rust-owned (engine_block_host.rs): routing
        // feedback, metering publication, the metronome, the master gain/limiter/
        // meter, the Control Room cue and automation recording. The playhead is an
        // atomic the control thread also reads, so the tail reports its step and
        // the callback applies it here.
        uint32_t playheadAdvance = 0;
        uint8_t rangeOverflow = 0;
        hirari_engine_block_finish(
            m_engineBlock, output.storageHandle(), off, sz, currentPlayhead,
            m_sampleRate.load(std::memory_order_relaxed),
            m_masterGain.load(std::memory_order_acquire),
            m_isPlaying.load(std::memory_order_acquire) ? 1u : 0u,
            m_autoRecordMode.load(std::memory_order_relaxed) > 0 ? 1u : 0u,
            m_talkbackInputOffset,
            reinterpret_cast<void* const*>(state.activeTracks), state.trackCount,
            &playheadAdvance, &rangeOverflow);
        if (playheadAdvance != 0) m_playhead.fetch_add(playheadAdvance, std::memory_order_relaxed);
        if (rangeOverflow != 0) m_audioRangeOverflow.store(true, std::memory_order_release);
    }

void HirariUnifiedEngine::processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples) {
    m_audioCallbacksInFlight.fetch_add(1, std::memory_order_acquire);
    struct CallbackGuard {
        std::atomic<uint32_t>& count;
        ~CallbackGuard() { count.fetch_sub(1, std::memory_order_release); }
    } callbackGuard{m_audioCallbacksInFlight};

    const bool validInput = buffers != nullptr && numChannels >= 2 && numChannels <= 32 &&
        numSamples > 0 && numSamples <= HirariUnifiedEngine::kMaxAudioBlockSize &&
        buffers[0] != nullptr && buffers[1] != nullptr;
    if (!validInput) {
        // The caller does not provide the actual allocation length.  When the
        // declared block size is invalid, writing even a bounded prefix could
        // overrun a shorter external buffer.  Fail closed without touching
        // memory whose extent cannot be proven at this boundary.
        hirari_input_blocks_end_talkback(m_inputBlocks);
        return;
    }
    for (uint32_t channel = 0; channel < numChannels; ++channel) {
        if (buffers[channel] == nullptr) return;
    }
    if (m_isShutdown.load(std::memory_order_acquire)) {
        AudioBuffer silence;
        silence.wrapChannels(buffers, numChannels, numSamples);
        silence.clear(numSamples);
        hirari_input_blocks_end_talkback(m_inputBlocks);
        return;
    }
    // Zero-Allocation Wrapper (Allocated on stack for absolute concurrency safety).
    // Track work buffers and the sandbox protocol are prepared for bounded
    // engine blocks.  A host/device callback may legally be larger than that
    // bound, so feed the same external buffer in bounded slices rather than
    // silently skipping every track whose prepared buffer is smaller.
    AudioBuffer wrap;
    wrap.wrapChannels(buffers, numChannels, numSamples);
    for (uint32_t offset = 0; offset < numSamples;) {
        // Rust owns the engine block cap and the cycle-boundary split so a
        // hardware callback never feeds a playhead past the loop end.
        const uint64_t plan = hirari_engine_chunk_plan(
            numSamples - offset, m_isPlaying.load(std::memory_order_acquire),
            m_loopEnabled.load(std::memory_order_acquire),
            m_cycleStart.load(std::memory_order_acquire),
            m_cycleEnd.load(std::memory_order_acquire),
            m_playhead.load(std::memory_order_relaxed));
        if ((plan >> 32) != 0) {
            m_playhead.store(m_cycleStart.load(std::memory_order_acquire), std::memory_order_relaxed);
            continue;
        }
        const uint32_t chunk = static_cast<uint32_t>(plan);
        if (chunk == 0) continue;
        m_inputMonitorBlockOffset = offset;
        m_talkbackInputOffset = offset;
        processBlock(wrap, offset, chunk);
        offset += chunk;
    }
    hirari_input_blocks_end_monitor(m_inputBlocks);
    hirari_input_blocks_end_talkback(m_inputBlocks);
}
