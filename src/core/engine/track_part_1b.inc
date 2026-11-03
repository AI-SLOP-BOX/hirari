public:
    uint32_t getId() const { return m_id; }
    void markProjectLayoutChanged() noexcept {
        if (m_layoutRevision) m_layoutRevision->markChanged();
    }

    const std::string& getName() const { return m_name; }
    void setName(std::string name) {
        if (!name.empty() && name != m_name) {
            m_name = std::move(name);
            markProjectLayoutChanged();
        }
    }
    Type getType() const { return m_type; }
    float getVolume() const noexcept { return m_volume.load(std::memory_order_acquire); }
    float getPan() const { return m_pan.load(std::memory_order_acquire); }
    bool isRecordArmed() const noexcept { return m_recordArmed.load(std::memory_order_acquire); }
    void setRecordArmed(bool armed) noexcept {
        if (m_recordArmed.exchange(armed, std::memory_order_acq_rel) != armed) {
            markProjectLayoutChanged();
        }
    }
    virtual uint32_t getTotalLatencySamples() const noexcept {
        return hirari_effect_chain_total_latency_samples(m_chain);
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
        const uint32_t next = std::min(samples, kMaxPdcDelaySamples);
        if (m_trackDelaySamples.exchange(next, std::memory_order_acq_rel) != next) {
            markProjectLayoutChanged();
        }
    }
    void setPdcCompensationSamples(uint32_t samples) noexcept {
        m_pdcRequested.store(std::min(samples, kMaxPdcDelaySamples),
                             std::memory_order_release);
    }
