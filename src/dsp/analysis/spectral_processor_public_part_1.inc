public:
    struct Rect { float t0, f0, t1, f1; }; // Time (sec) / Freq (Hz)
    struct RegionGain { Rect region{}; float gain = 1.0f; };

    // Offline spectral edits are user operations, so keep a bounded history
    // that can be driven directly by a UI command or the stable API.
    bool canUndo() const noexcept { return !m_undo.empty(); }
    bool canRedo() const noexcept { return !m_redo.empty(); }
    bool canUndo(const Core::AudioBuffer& buffer) const noexcept {
        return findLastOwned(m_undo, &buffer) != m_undo.end();
    }
    bool canRedo(const Core::AudioBuffer& buffer) const noexcept {
        return findLastOwned(m_redo, &buffer) != m_redo.end();
    }
    size_t undoDepth() const noexcept { return m_undo.size(); }
    size_t redoDepth() const noexcept { return m_redo.size(); }
    size_t undoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return countOwned(m_undo, &buffer);
    }
    size_t redoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return countOwned(m_redo, &buffer);
    }
    size_t historyBytes() const noexcept {
        size_t total = 0;
        for (const auto& item : m_undo) total += item.data.size() * sizeof(float);
        for (const auto& item : m_redo) total += item.data.size() * sizeof(float);
        return total;
    }
    const std::string& undoLabel() const noexcept {
        static const std::string empty;
        return m_undo.empty() ? empty : m_undo.back().label;
    }
    const std::string& redoLabel() const noexcept {
        static const std::string empty;
        return m_redo.empty() ? empty : m_redo.back().label;
    }
    const std::string& undoLabel(const Core::AudioBuffer& buffer) const noexcept {
        static const std::string empty;
        const auto it = findLastOwned(m_undo, &buffer);
        return it == m_undo.end() ? empty : it->label;
    }
    const std::string& redoLabel(const Core::AudioBuffer& buffer) const noexcept {
        static const std::string empty;
        const auto it = findLastOwned(m_redo, &buffer);
        return it == m_redo.end() ? empty : it->label;
    }

    bool undo(Core::AudioBuffer& buffer) {
        const auto it = findLastOwned(m_undo, &buffer);
        if (it == m_undo.end()) return false;
        Snapshot current;
        if (!makeSnapshot(buffer, current)) return false;
        current.label = it->label;
        if (!restoreSnapshot(buffer, *it)) return false;
        m_undo.erase(it);
        m_redo.push_back(std::move(current));
        trimHistory();
        return true;
    }

    bool redo(Core::AudioBuffer& buffer) {
        const auto it = findLastOwned(m_redo, &buffer);
        if (it == m_redo.end()) return false;
        Snapshot current;
        if (!makeSnapshot(buffer, current)) return false;
        current.label = it->label;
        if (!restoreSnapshot(buffer, *it)) return false;
        m_redo.erase(it);
        m_undo.push_back(std::move(current));
        trimHistory();
        return true;
    }

    void clearHistory() noexcept {
        m_undo.clear();
        m_redo.clear();
        m_groupHistory = false;
    }
    void clearHistory(const Core::AudioBuffer& buffer) noexcept {
        const auto owner = &buffer;
        std::erase_if(m_undo, [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        std::erase_if(m_redo, [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
    }

    void applyMask(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float gain) {
        applySpectralGain(buffer, sampleRate, target, gain);
    }

    // Non-destructive spectral gain edit. The inverse transform is accumulated
    // with a Hann window and only the edited delta is written back, preserving
    // phase continuity and samples outside the selection.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate, Rect target, float gain) {
        if (buffer.getNumSamples() == 0 || !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(gain)) return;
        if (!std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        if (!m_groupHistory) captureBefore(buffer, "Spectral Gain");
        if (target.t0 > target.t1) std::swap(target.t0, target.t1);
        if (target.f0 > target.f1) std::swap(target.f0, target.f1);
        target.t0 = std::max(0.0f, target.t0);
        target.t1 = std::max(target.t0, target.t1);
        target.f0 = std::max(0.0f, target.f0);
        target.f1 = std::max(target.f0, target.f1);
        gain = std::clamp(gain, 0.0f, 8.0f);

        const uint32_t fftSize = 2048;
        const uint32_t hop = fftSize / 2;
        std::vector<float> window(fftSize);
        std::vector<std::complex<float>> spectrum(fftSize);
        std::vector<float> originalTime(fftSize);
        std::vector<float> delta(buffer.getNumSamples(), 0.0f);
        std::vector<float> norm(buffer.getNumSamples(), 0.0f);
        for (uint32_t i = 0; i < fftSize; ++i) {
            const float phase = static_cast<float>(i) / static_cast<float>(fftSize - 1);
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
        }

        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            std::fill(delta.begin(), delta.end(), 0.0f);
            std::fill(norm.begin(), norm.end(), 0.0f);
            
            // Zero-pad edge frames so short clips and selections at the end of
            // a region are still editable (not silently skipped).
            const float frameSeconds = static_cast<float>(fftSize) / static_cast<float>(sampleRate);
            const float timeFeather = std::max(frameSeconds * 0.5f, 1.0e-4f);
            const float binWidth = static_cast<float>(sampleRate) / static_cast<float>(fftSize);
            const float freqFeather = std::max(binWidth * 2.0f, 1.0e-3f);
            const auto smoothstep = [](float x) noexcept {
                x = std::clamp(x, 0.0f, 1.0f);
                return x * x * (3.0f - 2.0f * x);
            };
            for (uint32_t offset = 0; offset < buffer.getNumSamples(); offset += hop) {
                const float timeSec = static_cast<float>(offset + fftSize / 2) / static_cast<float>(sampleRate);
                if (timeSec < target.t0 - timeFeather || timeSec > target.t1 + timeFeather) continue;
                const float timeWeight = smoothstep((timeSec - (target.t0 - timeFeather)) / timeFeather) *
                    smoothstep(((target.t1 + timeFeather) - timeSec) / timeFeather);
                if (timeWeight <= 0.0f) continue;

                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    const float sample = pos < buffer.getNumSamples() && std::isfinite(data[pos]) ? data[pos] : 0.0f;
                    originalTime[i] = sample * window[i];
                    spectrum[i] = {originalTime[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);

                for (uint32_t k = 0; k < fftSize; ++k) {
                    const float rawFreq = static_cast<float>(k) * static_cast<float>(sampleRate) / static_cast<float>(fftSize);
                    const float freq = std::min(rawFreq, static_cast<float>(sampleRate) - rawFreq);
                    if (freq < target.f0 - freqFeather || freq > target.f1 + freqFeather) continue;
                    const float lowWeight = smoothstep((freq - (target.f0 - freqFeather)) / freqFeather);
                    const float highWeight = smoothstep(((target.f1 + freqFeather) - freq) / freqFeather);
                    const float weight = timeWeight * lowWeight * highWeight;
                    spectrum[k] *= 1.0f + (gain - 1.0f) * weight;
                }
                
                Utils::FFTUtils::ifft(spectrum);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    if (pos >= buffer.getNumSamples()) break;
                    const float d = (spectrum[i].real() - originalTime[i]) * window[i];
                    if (std::isfinite(d)) delta[pos] += d;
                    norm[pos] += window[i] * window[i];
                }
            }
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float d = norm[i] > 1.0e-6f ? delta[i] / norm[i] : 0.0f;
                if (std::isfinite(d)) data[i] += d;
            }
        }
    }

    // Apply one spectral edit to a set of disjoint canvas regions.  UI lasso
    // and brush tools can rasterize their mask into rectangles and submit it
    // as one atomic operation, so undo restores the complete edit rather than
    // exposing one history item per raster cell.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<Rect>& regions, float gain) {
        if (regions.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(gain)) return;
        bool hasValidRegion = false;
        for (const auto& region : regions) {
            if (std::isfinite(region.t0) && std::isfinite(region.t1) &&
                std::isfinite(region.f0) && std::isfinite(region.f1)) {
                hasValidRegion = true;
                break;
            }
        }
        if (!hasValidRegion) return;

        captureBefore(buffer, "Spectral Gain (Multi-Region)");
        m_groupHistory = true;
        try {
            for (const auto& region : regions)
                applySpectralGain(buffer, sampleRate, region, gain);
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
    }

    // Apply a variable-strength spectral brush mask. Each region can have a
    // distinct normalized gain (0=mute, 1=unchanged, >1=boost), while the
    // entire brush stroke remains one atomic undo operation.
    void applySpectralMask(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<RegionGain>& mask) {
        if (mask.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0) return;
        bool hasValidRegion = false;
        for (const auto& item : mask) {
            if (std::isfinite(item.region.t0) && std::isfinite(item.region.t1) &&
                std::isfinite(item.region.f0) && std::isfinite(item.region.f1) &&
                std::isfinite(item.gain)) {
                hasValidRegion = true;
                break;
            }
        }
        if (!hasValidRegion) return;
        captureBefore(buffer, "Spectral Brush");
        m_groupHistory = true;
        try {
            for (const auto& item : mask) {
                if (std::isfinite(item.region.t0) && std::isfinite(item.region.t1) &&
                    std::isfinite(item.region.f0) && std::isfinite(item.region.f1) &&
                    std::isfinite(item.gain))
                    applySpectralGain(buffer, sampleRate, item.region, item.gain);
            }
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
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
            !std::isfinite(bandwidthHz) || bandwidthHz <= 0.0f || harmonics == 0) return;
        if (!std::isfinite(selection.t0) || !std::isfinite(selection.t1) ||
            !std::isfinite(selection.f0) || !std::isfinite(selection.f1)) return;
        if (buffer.getNumSamples() == 0 || buffer.getNumChannels() == 0) return;
        if (selection.t0 > selection.t1) std::swap(selection.t0, selection.t1);
        if (selection.f0 > selection.f1) std::swap(selection.f0, selection.f1);
        selection.t0 = std::max(0.0f, selection.t0);
        selection.t1 = std::max(selection.t0, selection.t1);
        selection.f0 = std::max(0.0f, selection.f0);
        selection.f1 = std::max(selection.f0, selection.f1);
        captureBefore(buffer, "Remove Hum");
        m_groupHistory = true;
        const float nyquist = static_cast<float>(sampleRate * 0.5);
        try {
            for (uint32_t h = 1; h <= std::min<uint32_t>(harmonics, 32u); ++h) {
                const float f = fundamentalHz * static_cast<float>(h);
                if (f >= nyquist) break;
                const float low = std::max(selection.f0, f - bandwidthHz);
                const float high = std::min(selection.f1, f + bandwidthHz);
                if (high > low) applySpectralGain(buffer, sampleRate,
                                                   Rect{selection.t0, low, selection.t1, high}, 0.0f);
            }
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
    }

    // Learn a stationary noise bed from the leading portion of a clip and
    // apply bounded spectral subtraction. When target carries a finite time
    // or frequency range, only that Spectral Canvas selection is edited. This
    // is an offline restoration operation and keeps the original phase.
    void reduceNoise(Core::AudioBuffer& buffer, double sampleRate,
                     float amount = 1.0f, float profileSeconds = 0.5f,
                     Rect target = Rect{0.0f, 0.0f, 1.0e9f, 0.0f}) {
        if (buffer.getNumSamples() < 64 || buffer.getNumChannels() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0) return;
        amount = std::clamp(std::isfinite(amount) ? amount : 1.0f, 0.0f, 1.0f);
        profileSeconds = std::clamp(std::isfinite(profileSeconds) ? profileSeconds : 0.5f,
                                    0.02f, 10.0f);
        if (!std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        if (target.t0 > target.t1) std::swap(target.t0, target.t1);
        if (target.f0 > target.f1) std::swap(target.f0, target.f1);
        target.t0 = std::max(0.0f, target.t0);
        target.t1 = std::max(target.t0, target.t1);
        target.f0 = std::max(0.0f, target.f0);
        target.f1 = std::max(target.f0, target.f1);
        captureBefore(buffer, "Reduce Noise");
        const uint32_t fftSize = 2048;
        const uint32_t hop = fftSize / 2;
        const float timeFeather = std::max(static_cast<float>(fftSize) /
                                           static_cast<float>(sampleRate) * 0.5f, 1.0e-4f);
        const float binWidth = static_cast<float>(sampleRate) / static_cast<float>(fftSize);
        const float freqFeather = std::max(binWidth * 2.0f, 1.0e-3f);
        const bool restrictFrequency = target.f1 > target.f0;
        const auto smoothstep = [](float x) noexcept {
            x = std::clamp(x, 0.0f, 1.0f);
            return x * x * (3.0f - 2.0f * x);
        };
        std::vector<float> window(fftSize);
        std::vector<std::complex<float>> spectrum(fftSize);
        std::vector<float> profile(fftSize, 0.0f);
        for (uint32_t i = 0; i < fftSize; ++i) {
            const float phase = static_cast<float>(i) / static_cast<float>(fftSize - 1);
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
        }
        const uint32_t profileFrames = std::max<uint32_t>(1u,
            static_cast<uint32_t>(profileSeconds * sampleRate / hop));
        // Build one linked noise profile from all available channels. Applying
        // a common floor keeps stereo/immersive image stable instead of
        // modulating each channel independently.
        uint32_t learned = 0;
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            const float* data = buffer.getReadPointer(c);
            if (!data) continue;
            uint32_t channelFrames = 0;
            for (uint32_t offset = 0; offset < buffer.getNumSamples() && channelFrames < profileFrames; offset += hop, ++channelFrames) {
