private:
    static uintptr_t ownerKey(const Core::AudioBuffer& buffer) noexcept {
        return reinterpret_cast<uintptr_t>(&buffer);
    }

    const std::string& historyLabel(uintptr_t owner, bool redo) const noexcept {
        m_labelCache.clear();
        const size_t length = hirari_spectral_history_label(m_history, owner, redo, nullptr, 0);
        try { m_labelCache.resize(length); }
        catch (...) { return m_labelCache; }
        if (length > 0) {
            hirari_spectral_history_label(m_history, owner, redo,
                reinterpret_cast<uint8_t*>(m_labelCache.data()), length);
        }
        return m_labelCache;
    }

    void captureBefore(const Core::AudioBuffer& buffer, const char* label) {
        const char* safeLabel = label ? label : "Spectral Edit";
        (void)hirari_spectral_history_capture(m_history, ownerKey(buffer),
            buffer.getNumChannels(), buffer.getNumSamples(), buffer.getArrayOfReadPointers(),
            reinterpret_cast<const uint8_t*>(safeLabel), std::strlen(safeLabel));
    }

    bool restoreHistory(Core::AudioBuffer& buffer, bool redo) {
        const uintptr_t owner = ownerKey(buffer);
        void* target = hirari_spectral_history_peek(m_history, owner, redo);
        if (!target) return false;
        std::unique_ptr<void, decltype(&hirari_spectral_history_token_destroy)>
            targetGuard(target, &hirari_spectral_history_token_destroy);
        const uint32_t channels = hirari_spectral_history_snapshot_channels(m_history, target);
        const uint32_t frames = hirari_spectral_history_snapshot_frames(m_history, target);
        const size_t labelLength = hirari_spectral_history_snapshot_label(m_history, target, nullptr, 0);
        std::string label;
        try { label.resize(labelLength); }
        catch (...) { return false; }
        if (labelLength > 0) {
            hirari_spectral_history_snapshot_label(m_history, target,
                reinterpret_cast<uint8_t*>(label.data()), labelLength);
        }
        void* current = hirari_spectral_history_snapshot_create(owner,
            buffer.getNumChannels(), buffer.getNumSamples(), buffer.getArrayOfReadPointers(),
            reinterpret_cast<const uint8_t*>(label.data()), label.size());
        if (!current) return false;
        std::unique_ptr<void, decltype(&hirari_spectral_history_snapshot_destroy)>
            currentGuard(current, &hirari_spectral_history_snapshot_destroy);
        if ((buffer.getNumChannels() != channels || buffer.getNumSamples() != frames) &&
            !buffer.resize(channels, frames)) return false;
        for (uint32_t c = 0; c < channels; ++c) {
            if (!buffer.getWritePointer(c)) return false;
        }
        if (!hirari_spectral_history_snapshot_restore(m_history, target, buffer.getArrayOfWritePointers(),
                                                       channels, frames)) return false;
        if (!hirari_spectral_history_finish_restore(m_history, owner, redo,
                                                    target, current)) return false;
        targetGuard.release();
        currentGuard.release();
        return true;
    }

    void* m_history = hirari_spectral_history_create();
    mutable std::string m_labelCache;
};
