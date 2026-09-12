    uint64_t getHeartbeat() const {
#if !defined(_WIN32)
        return m_shared != nullptr ? m_shared->heartbeat.load(std::memory_order_acquire) : 0;
#else
        return m_heartbeat.load(std::memory_order_acquire);
#endif
    }

    uint32_t takeDroppedOutputMidi() noexcept {
#if defined(_WIN32)
        return 0;
#else
        return m_shared ? m_shared->outputMidiDropped.exchange(0, std::memory_order_acq_rel) : 0;
#endif
    }

    uint32_t takeMailboxOverruns() noexcept {
        return m_shared ? m_shared->mailboxOverruns.exchange(0, std::memory_order_acq_rel) : 0;
    }

    uint32_t takeInputMidiTruncations() noexcept {
        return m_shared ? m_shared->inputMidiTruncations.exchange(0, std::memory_order_acq_rel) : 0;
    }

    void recordInputMidiRejection() noexcept {
        if (m_shared) m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
    }

    bool takeLastProcessOverrun() noexcept {
        return m_lastProcessOverrun.exchange(false, std::memory_order_acq_rel);
    }

    // Audio-thread only. The completion call must be able to tell the
    // processor that the block it just collected had previously missed its
    // deadline; otherwise a slow block would increment the consecutive
    // overrun counter and immediately reset it on completion.
    bool takeCompletedSequenceOverrun() noexcept {
        const bool value = m_completedSequenceHadOverrun;
        m_completedSequenceHadOverrun = false;
        return value;
    }

    uint32_t activeSampleRate() const noexcept {
        return m_shared ? m_shared->activeSampleRate.load(std::memory_order_acquire) : 0;
    }

    uint32_t activeChannels() const noexcept {
        return m_shared ? m_shared->activeChannels.load(std::memory_order_acquire) : 0;
    }

    uint32_t takeProcessErrors() noexcept {
#if defined(_WIN32)
        return 0;
#else
        return m_shared ? m_shared->processErrors.exchange(0, std::memory_order_acq_rel) : 0;
#endif
    }

    // Control-thread only. State IPC is bounded and has a finite timeout;
    // it is never used by the real-time process() path.
