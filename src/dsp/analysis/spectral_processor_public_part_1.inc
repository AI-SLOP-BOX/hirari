public:
    struct Rect { float t0, f0, t1, f1; }; // Time (sec) / Freq (Hz)
    struct RegionGain { Rect region{}; float gain = 1.0f; };

    // Offline snapshots and bounded undo/redo history are owned by Rust.
    bool canUndo() const noexcept { return hirari_spectral_history_contains(m_history, 0, false); }
    bool canRedo() const noexcept { return hirari_spectral_history_contains(m_history, 0, true); }
    bool canUndo(const Core::AudioBuffer& buffer) const noexcept {
        return hirari_spectral_history_contains(m_history, ownerKey(buffer), false);
    }
    bool canRedo(const Core::AudioBuffer& buffer) const noexcept {
        return hirari_spectral_history_contains(m_history, ownerKey(buffer), true);
    }
    size_t undoDepth() const noexcept { return hirari_spectral_history_depth(m_history, 0, false); }
    size_t redoDepth() const noexcept { return hirari_spectral_history_depth(m_history, 0, true); }
    size_t undoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return hirari_spectral_history_depth(m_history, ownerKey(buffer), false);
    }
    size_t redoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return hirari_spectral_history_depth(m_history, ownerKey(buffer), true);
    }
    size_t historyBytes() const noexcept { return hirari_spectral_history_bytes(m_history); }
    const std::string& undoLabel() const noexcept { return historyLabel(0, false); }
    const std::string& redoLabel() const noexcept { return historyLabel(0, true); }
    const std::string& undoLabel(const Core::AudioBuffer& buffer) const noexcept {
        return historyLabel(ownerKey(buffer), false);
    }
    const std::string& redoLabel(const Core::AudioBuffer& buffer) const noexcept {
        return historyLabel(ownerKey(buffer), true);
    }

    bool undo(Core::AudioBuffer& buffer) { return restoreHistory(buffer, false); }
    bool redo(Core::AudioBuffer& buffer) { return restoreHistory(buffer, true); }

    void clearHistory() noexcept { hirari_spectral_history_clear(m_history, 0); }
    void clearHistory(const Core::AudioBuffer& buffer) noexcept {
        hirari_spectral_history_clear(m_history, ownerKey(buffer));
    }

    void applyMask(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float gain) {
        applySpectralGain(buffer, sampleRate, target, gain);
    }

    // Non-destructive spectral gain edit. The inverse transform is accumulated
    // with a Hann window and only the edited delta is written back, preserving
    // phase continuity and samples outside the selection.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate, Rect target, float gain) {
        if (buffer.getNumSamples() == 0 || !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(gain) ||
            !std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        captureBefore(buffer, "Spectral Gain");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_apply_gain(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            target.t0, target.f0, target.t1, target.f1, gain);
    }

    // Apply one spectral edit to a set of disjoint canvas regions.  UI lasso
    // and brush tools can rasterize their mask into rectangles and submit it
    // as one atomic operation, so undo restores the complete edit rather than
    // exposing one history item per raster cell.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<Rect>& regions, float gain) {
        if (regions.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(gain)) return;
        std::vector<float> flattened;
        flattened.reserve(regions.size() * 4);
        for (const auto& region : regions) {
            if (!std::isfinite(region.t0) || !std::isfinite(region.t1) ||
                !std::isfinite(region.f0) || !std::isfinite(region.f1)) continue;
            flattened.insert(flattened.end(), {region.t0, region.f0, region.t1, region.f1});
        }
        if (flattened.empty()) return;
        captureBefore(buffer, "Spectral Gain (Multi-Region)");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_apply_gain_regions(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            flattened.data(), static_cast<uint32_t>(flattened.size() / 4), gain);
    }

    // Apply a variable-strength spectral brush mask. Each region can have a
    // distinct normalized gain (0=mute, 1=unchanged, >1=boost), while the
    // entire brush stroke remains one atomic undo operation.
    void applySpectralMask(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<RegionGain>& mask) {
        if (mask.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0) return;
        std::vector<float> flattened;
        std::vector<float> gains;
        flattened.reserve(mask.size() * 4);
        gains.reserve(mask.size());
        for (const auto& item : mask) {
            if (!std::isfinite(item.region.t0) || !std::isfinite(item.region.t1) ||
                !std::isfinite(item.region.f0) || !std::isfinite(item.region.f1) ||
                !std::isfinite(item.gain)) continue;
            flattened.insert(flattened.end(), {item.region.t0, item.region.f0,
                                                item.region.t1, item.region.f1});
            gains.push_back(item.gain);
        }
        if (gains.empty()) return;
        captureBefore(buffer, "Spectral Brush");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_apply_mask(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            flattened.data(), gains.data(), static_cast<uint32_t>(gains.size()));
    }

    // Remove mains hum and its harmonics in a selected time range.
    void removeHum(Core::AudioBuffer& buffer, double sampleRate, float fundamentalHz,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        removeHum(buffer, sampleRate, fundamentalHz, Rect{0.0f, 0.0f, 1.0e9f,
                                                           static_cast<float>(sampleRate * 0.5)},
                  harmonics, bandwidthHz);
    }

    // Remove mains hum only inside a selected time/frequency canvas region.
    // The frequency bounds are intersected with each harmonic band, allowing
    // a UI lasso to leave unrelated low-frequency content untouched.
    void removeHum(Core::AudioBuffer& buffer, double sampleRate, float fundamentalHz,
                   Rect selection, uint32_t harmonics = 8,
                   float bandwidthHz = 3.0f) {
        if (!std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(fundamentalHz) || fundamentalHz <= 0.0f ||
            !std::isfinite(bandwidthHz) || bandwidthHz <= 0.0f || harmonics == 0 ||
            buffer.getNumSamples() == 0 || buffer.getNumChannels() == 0 ||
            !std::isfinite(selection.t0) || !std::isfinite(selection.t1) ||
            !std::isfinite(selection.f0) || !std::isfinite(selection.f1)) return;
        if (selection.t0 > selection.t1) std::swap(selection.t0, selection.t1);
        if (selection.f0 > selection.f1) std::swap(selection.f0, selection.f1);
        selection.t0 = std::max(0.0f, selection.t0);
        selection.t1 = std::max(selection.t0, selection.t1);
        selection.f0 = std::max(0.0f, selection.f0);
        selection.f1 = std::max(selection.f0, selection.f1);
        captureBefore(buffer, "Remove Hum");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_remove_hum(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            fundamentalHz, harmonics, bandwidthHz, selection.t0, selection.f0,
            selection.t1, selection.f1);
    }

    // Learn a stationary noise bed from the leading portion of a clip and
    // apply bounded spectral subtraction. When target carries a finite time
    // or frequency range, only that Spectral Canvas selection is edited. This
    // is an offline restoration operation and keeps the original phase.
    void reduceNoise(Core::AudioBuffer& buffer, double sampleRate,
                     float amount = 1.0f, float profileSeconds = 0.5f,
                     Rect target = Rect{0.0f, 0.0f, 1.0e9f, 0.0f}) {
        if (buffer.getNumSamples() < 64 || buffer.getNumChannels() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        amount = std::clamp(std::isfinite(amount) ? amount : 1.0f, 0.0f, 1.0f);
        profileSeconds = std::clamp(std::isfinite(profileSeconds) ? profileSeconds : 0.5f,
                                    0.02f, 10.0f);
        captureBefore(buffer, "Reduce Noise");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_reduce_noise(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            target.t0, target.f0, target.t1, target.f1, amount, profileSeconds);
    }
