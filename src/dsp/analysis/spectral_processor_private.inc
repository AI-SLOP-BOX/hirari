private:
    struct Snapshot {
        const Core::AudioBuffer* owner = nullptr;
        uint32_t channels = 0;
        uint32_t samples = 0;
        std::string label;
        std::vector<float> data;
    };

    static constexpr size_t kMaxHistory = 16;
    static constexpr size_t kMaxSnapshotSamples = 16u * 1024u * 1024u;
    static constexpr size_t kMaxHistoryBytes = 256u * 1024u * 1024u;

    using History = std::deque<Snapshot>;
    static History::const_iterator findLastOwned(const History& history,
                                                  const Core::AudioBuffer* owner) noexcept {
        const auto reverse = std::find_if(history.rbegin(), history.rend(),
                                          [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        return reverse == history.rend() ? history.end() : std::prev(reverse.base());
    }
    static History::iterator findLastOwned(History& history,
                                           const Core::AudioBuffer* owner) noexcept {
        const auto reverse = std::find_if(history.rbegin(), history.rend(),
                                          [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        return reverse == history.rend() ? history.end() : std::prev(reverse.base());
    }
    static size_t countOwned(const History& history,
                             const Core::AudioBuffer* owner) noexcept {
        return static_cast<size_t>(std::count_if(
            history.begin(), history.end(),
            [owner](const Snapshot& snapshot) { return snapshot.owner == owner; }));
    }

    static bool makeSnapshot(const Core::AudioBuffer& buffer, Snapshot& out) {
        const uint64_t count = static_cast<uint64_t>(buffer.getNumChannels()) * buffer.getNumSamples();
        if (count > kMaxSnapshotSamples) return false;
        out.channels = buffer.getNumChannels();
        out.samples = buffer.getNumSamples();
        out.owner = &buffer;
        try { out.data.resize(static_cast<size_t>(count)); }
        catch (...) { return false; }
        size_t at = 0;
        for (uint32_t c = 0; c < out.channels; ++c) {
            const float* source = buffer.getReadPointer(c);
            if (!source) return false;
            std::copy(source, source + out.samples, out.data.begin() + static_cast<std::ptrdiff_t>(at));
            at += out.samples;
        }
        return true;
    }

    static bool restoreSnapshot(Core::AudioBuffer& buffer, const Snapshot& snapshot) {
        if (snapshot.channels == 0 || snapshot.samples == 0 ||
            snapshot.data.size() != static_cast<size_t>(snapshot.channels) * snapshot.samples) return false;
        if (buffer.getNumChannels() != snapshot.channels || buffer.getNumSamples() != snapshot.samples) {
            if (!buffer.resize(snapshot.channels, snapshot.samples)) return false;
        }
        size_t at = 0;
        for (uint32_t c = 0; c < snapshot.channels; ++c) {
            float* destination = buffer.getWritePointer(c);
            if (!destination) return false;
            std::copy(snapshot.data.begin() + static_cast<std::ptrdiff_t>(at),
                      snapshot.data.begin() + static_cast<std::ptrdiff_t>(at + snapshot.samples), destination);
            at += snapshot.samples;
        }
        return true;
    }

    void captureBefore(const Core::AudioBuffer& buffer, const char* label) {
        Snapshot snapshot;
        if (!makeSnapshot(buffer, snapshot)) return;
        snapshot.label = label ? label : "Spectral Edit";
        if (m_undo.size() >= kMaxHistory) m_undo.pop_front();
        m_undo.push_back(std::move(snapshot));
        m_redo.clear();
        trimHistory();
    }

    void trimHistory() noexcept {
        while (historyBytes() > kMaxHistoryBytes && (!m_undo.empty() || !m_redo.empty())) {
            if (!m_undo.empty()) m_undo.pop_front();
            else m_redo.pop_front();
        }
    }

    History m_undo;
    History m_redo;
    bool m_groupHistory = false;
};
