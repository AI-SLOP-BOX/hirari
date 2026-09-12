private:
    static constexpr size_t kMaxStateBytes = 4u * 1024u * 1024u;
    static constexpr uint32_t kMaxBlockSize = 4096;
    static constexpr OSStatus kStateTooLargeError = -2;
    static constexpr OSStatus kStateAllocationError = -3;
    static constexpr OSStatus kStateUnavailableError = -4;
    static constexpr OSStatus kStateDecodeError = -5;
    static constexpr OSStatus kInvalidAudioConfigurationError = -6;

    void beginStateTransition() noexcept {
        // Preserve a user/watchdog bypass state. A state restore must not
        // accidentally re-enable a plugin that was already quarantined.
        m_transitionPreviousBypass = m_bypassed.exchange(true, std::memory_order_acq_rel);
        m_stateTransition.store(true, std::memory_order_release);
        waitForQuiescence();
    }

    void endStateTransition() noexcept {
        m_stateTransition.store(false, std::memory_order_release);
        m_bypassed.store(m_transitionPreviousBypass, std::memory_order_release);
    }

    void waitForQuiescence() noexcept {
        while (m_activeProcesses.load(std::memory_order_acquire) != 0) {
            std::this_thread::yield();
        }
    }

    OSStatus renderAudioUnit(
        AudioUnitRenderActionFlags* flags,
        const AudioTimeStamp* timeStamp,
        UInt32 outputBus,
        UInt32 numberFrames,
        AudioBufferList* bufferList) noexcept {
        return AudioUnitRender(m_auInstance, flags, timeStamp, outputBus, numberFrames, bufferList);
    }

    AudioUnit m_auInstance;
    AudioBufferList* m_abl = nullptr;
    std::atomic<bool> m_bypassed{false};
    std::atomic<uint32_t> m_numChannels{2};
    std::atomic<OSStatus> m_lastError{noErr};
    std::atomic<LoadState> m_state;
    std::atomic<bool> m_shuttingDown{true};
    std::atomic<bool> m_stateTransition{false};
    std::atomic<uint32_t> m_activeProcesses{0};
    static constexpr OSStatus componentNotFoundError = -1;
    static constexpr uint32_t kWatchdogOverrunLimit = 3;
    uint32_t m_watchdogOverruns = 0;
    std::atomic<bool> m_watchdogTripPending{false};
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 1024;
    std::atomic<ProcessFunction> m_processFunction{nullptr};
    std::atomic<bool> m_processFailed{false};
    std::atomic<bool> m_parameterScheduleFailed{false};
    std::atomic<uint64_t> m_parameterQueueOverruns{0};
    std::atomic<uint64_t> m_nonFiniteSamples{0};
    std::atomic<bool> m_stateRestoreFailed{false};
    bool m_transitionPreviousBypass = false;
#if defined(__APPLE__)
    CFBundleRef m_cocoaBundle = nullptr;
    void* m_nativeEditorFactory = nullptr;
    void* m_nativeEditorView = nullptr;
#endif

    struct ParameterCmd { uint32_t id; float value; uint32_t offsetSamples; };

    // --- INDUSTRIAL LOCK-FREE QUEUE ---
    template<typename T, uint32_t Size>
    struct RTQueue {
        T buffer[Size];
        std::atomic<uint32_t> writePtr{0}, readPtr{0};
        bool push(const T& val) {
            uint32_t w = writePtr.load(std::memory_order_relaxed);
            uint32_t r = readPtr.load(std::memory_order_acquire);
            if (w - r >= Size) return false;
            buffer[w % Size] = val;
            writePtr.store(w + 1, std::memory_order_release);
            return true;
        }
        bool pop(T& val) {
            uint32_t r = readPtr.load(std::memory_order_relaxed);
            if (r == writePtr.load(std::memory_order_acquire)) return false;
            val = buffer[r % Size];
            readPtr.store(r + 1, std::memory_order_release);
            return true;
        }
    };

    RTQueue<ParameterCmd, 256> m_paramQueue;

    alignas(AudioBufferList) uint8_t m_ablStorage[sizeof(AudioBufferList) + (sizeof(AudioBuffer) * (MaxChannels - 1))];
};
