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
        void* silence = hirari_audio_buffer_storage_create();
        (void)hirari_audio_buffer_storage_wrap_channels(
            silence, buffers, numChannels, numSamples);
        hirari_audio_buffer_storage_clear(silence, 0, numSamples);
        hirari_audio_buffer_storage_destroy(silence);
        hirari_input_blocks_end_talkback(m_inputBlocks);
        return;
    }
    // Zero-Allocation Wrapper (Allocated on stack for absolute concurrency safety).
    // Track work buffers and the sandbox protocol are prepared for bounded
    // engine blocks.  A host/device callback may legally be larger than that
    // bound, so feed the same external buffer in bounded slices rather than
    // silently skipping every track whose prepared buffer is smaller.
    void* wrap = hirari_audio_buffer_storage_create();
    (void)hirari_audio_buffer_storage_wrap_channels(
        wrap, buffers, numChannels, numSamples);
    for (uint32_t offset = 0; offset < numSamples;) {
        // Rust owns the engine block cap, the transport and the cycle-boundary
        // split, including the rewind a playhead that reached the loop end needs.
        const uint64_t plan = hirari_engine_block_chunk_plan(m_engineBlock, numSamples - offset);
        if ((plan >> 32) != 0) continue;
        const uint32_t chunk = static_cast<uint32_t>(plan);
        if (chunk == 0) continue;
        processBlock(wrap, offset, chunk);
        offset += chunk;
    }
    hirari_audio_buffer_storage_destroy(wrap);
    hirari_input_blocks_end_monitor(m_inputBlocks);
    hirari_input_blocks_end_talkback(m_inputBlocks);
}
