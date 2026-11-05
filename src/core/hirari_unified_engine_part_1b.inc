void HirariUnifiedEngine::processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples) {
    hirari_engine_block_callback_enter(handle(kHirariHandleEngineBlock));
    struct CallbackGuard {
        void* state;
        ~CallbackGuard() { hirari_engine_block_callback_leave(state); }
    } callbackGuard{handle(kHirariHandleEngineBlock)};

    const bool validInput = buffers != nullptr && numChannels >= 2 && numChannels <= 32 &&
        numSamples > 0 && numSamples <= HirariUnifiedEngine::kMaxAudioBlockSize &&
        buffers[0] != nullptr && buffers[1] != nullptr;
    if (!validInput) {
        // The caller does not provide the actual allocation length.  When the
        // declared block size is invalid, writing even a bounded prefix could
        // overrun a shorter external buffer.  Fail closed without touching
        // memory whose extent cannot be proven at this boundary.
        hirari_input_blocks_end_talkback(handle(kHirariHandleInputBlocks));
        return;
    }
    for (uint32_t channel = 0; channel < numChannels; ++channel) {
        if (buffers[channel] == nullptr) return;
    }
    if (hirari_engine_block_is_shutdown(handle(kHirariHandleEngineBlock)) != 0) {
        void* silence = hirari_audio_buffer_storage_create();
        (void)hirari_audio_buffer_storage_wrap_channels(
            silence, buffers, numChannels, numSamples);
        hirari_audio_buffer_storage_clear(silence, 0, numSamples);
        hirari_audio_buffer_storage_destroy(silence);
        hirari_input_blocks_end_talkback(handle(kHirariHandleInputBlocks));
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
        const uint64_t plan = hirari_engine_block_chunk_plan(handle(kHirariHandleEngineBlock), numSamples - offset);
        if ((plan >> 32) != 0) continue;
        const uint32_t chunk = static_cast<uint32_t>(plan);
        if (chunk == 0) continue;
        processBlock(wrap, offset, chunk);
        offset += chunk;
    }
    hirari_audio_buffer_storage_destroy(wrap);
    hirari_input_blocks_end_monitor(handle(kHirariHandleInputBlocks));
    hirari_input_blocks_end_talkback(handle(kHirariHandleInputBlocks));
}
