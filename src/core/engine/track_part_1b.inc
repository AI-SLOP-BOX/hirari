                left[i] = std::isfinite(left[i] * vcaGain) ? left[i] * vcaGain : 0.0f;
                right[i] = std::isfinite(right[i] * vcaGain) ? right[i] * vcaGain : 0.0f;
            }
        }
        if (buffer.getNumChannels() >= 2) {
            float* left = buffer.getWritePointer(0);
            float* right = buffer.getWritePointer(1);
            if (!std::isfinite(m_lowStateL)) m_lowStateL = 0.0f;
            if (!std::isfinite(m_lowStateR)) m_lowStateR = 0.0f;
            if (!std::isfinite(m_lowGain)) m_lowGain = 1.0f;
            if (!std::isfinite(m_highGain)) m_highGain = 1.0f;
            for (uint32_t i = 0; i < sz; ++i) {
                const float inL = std::isfinite(left[i]) ? std::clamp(left[i], -1.0e6f, 1.0e6f) : 0.0f;
                const float inR = std::isfinite(right[i]) ? std::clamp(right[i], -1.0e6f, 1.0e6f) : 0.0f;
                m_lowStateL += 0.02f * (inL - m_lowStateL);
                m_lowStateR += 0.02f * (inR - m_lowStateR);
                const float outL = m_lowStateL * m_lowGain + (inL - m_lowStateL) * m_highGain;
                const float outR = m_lowStateR * m_lowGain + (inR - m_lowStateR) * m_highGain;
                if (std::isfinite(outL)) {
                    left[i] = outL;
                } else {
                    left[i] = 0.0f;
                    m_lowStateL = 0.0f;
                }
                if (std::isfinite(outR)) {
                    right[i] = outR;
                } else {
                    right[i] = 0.0f;
                    m_lowStateR = 0.0f;
                }
            }
        }
    }

public:
    uint32_t getId() const { return m_id; }

    uint64_t getEndSample() const {
        uint64_t end = 0;
        for (const auto& region : getRegions()) {
            if (region.start > UINT64_MAX - region.len) return UINT64_MAX;
            end = std::max(end, region.start + region.len);
        }
        return end;
    }
    const std::string& getName() const { return m_name; }
    void setName(std::string name) {
        if (!name.empty()) m_name = std::move(name);
    }
    Type getType() const { return m_type; }
    float getVolume() const noexcept { return m_volume.load(std::memory_order_acquire); }
    float getPan() const { return m_pan.load(std::memory_order_acquire); }
    bool isRecordArmed() const noexcept { return m_recordArmed.load(std::memory_order_acquire); }
    void setRecordArmed(bool armed) noexcept { m_recordArmed.store(armed, std::memory_order_release); }
    virtual uint32_t getTotalLatencySamples() const noexcept {
        return m_effectChain.getTotalLatencySamples();
    }
    uint32_t getTotalTailSamples() const noexcept {
        return m_effectChain.getTotalTailSamples();
    }
    uint32_t getPdcCompensationSamples() const noexcept {
        return m_pdcRequested.load(std::memory_order_acquire);
    }

    // User timing offset is intentionally separate from plugin PDC.  Both
    // values use the same preallocated transition line, but a graph rebuild
    // must never erase the musician's manual delay.
    uint32_t getTrackDelaySamples() const noexcept {
        return m_trackDelaySamples.load(std::memory_order_acquire);
    }
    void setTrackDelaySamples(uint32_t samples) noexcept {
        m_trackDelaySamples.store(std::min(samples, kMaxPdcDelaySamples),
                                  std::memory_order_release);
    }
    void setPdcCompensationSamples(uint32_t samples) noexcept {
        m_pdcRequested.store(std::min(samples, kMaxPdcDelaySamples),
                             std::memory_order_release);
    }

    void applyPdcCompensation(AudioBuffer& buffer, uint32_t sz) noexcept {
        if (sz == 0 || buffer.getNumChannels() < 2) return;
        const uint32_t pdc = m_pdcRequested.load(std::memory_order_acquire);
        const uint32_t manual = m_trackDelaySamples.load(std::memory_order_acquire);
        const uint32_t requested = std::min(kMaxPdcDelaySamples,
            pdc > kMaxPdcDelaySamples - manual ? kMaxPdcDelaySamples : pdc + manual);
        if (requested != m_pdcActive) {
            // Keep the existing history. Clearing the line here creates a
            // full-block discontinuity whenever a plugin reports a new
            // latency. Crossfade the old and new tap instead.
            m_pdcPrevious = m_pdcActive;
            m_pdcActive = requested;
            m_pdcTransitionRemaining = 64;
        }
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getWritePointer(1);
        constexpr uint32_t capacity = kMaxPdcDelaySamples + 1;
        for (uint32_t i = 0; i < sz; ++i) {
            const float inL = left[i];
            const float inR = right[i];
            const auto delayed = [this](uint32_t delay, bool rightChannel) {
                if (delay == 0) return 0.0f;
                const uint32_t read = (m_pdcWrite + capacity - delay) % capacity;
                return rightChannel ? m_pdcDelayR[read] : m_pdcDelayL[read];
            };
            const float oldL = m_pdcPrevious == 0 ? inL : delayed(m_pdcPrevious, false);
            const float oldR = m_pdcPrevious == 0 ? inR : delayed(m_pdcPrevious, true);
            const float newL = m_pdcActive == 0 ? inL : delayed(m_pdcActive, false);
            const float newR = m_pdcActive == 0 ? inR : delayed(m_pdcActive, true);
            if (m_pdcTransitionRemaining > 0) {
                const float progress = 1.0f -
                    static_cast<float>(m_pdcTransitionRemaining) / 64.0f;
                left[i] = oldL + (newL - oldL) * progress;
                right[i] = oldR + (newR - oldR) * progress;
                --m_pdcTransitionRemaining;
            } else {
                left[i] = newL;
                right[i] = newR;
            }
            m_pdcDelayL[m_pdcWrite] = std::isfinite(inL) ? inL : 0.0f;
            m_pdcDelayR[m_pdcWrite] = std::isfinite(inR) ? inR : 0.0f;
            m_pdcWrite = (m_pdcWrite + 1) % capacity;
        }
    }
