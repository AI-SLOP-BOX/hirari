public:
    static constexpr uint32_t MaxChannels = 128;
    enum class LoadState : uint8_t { Unloaded, InstanceCreated, Operational, Failed };
    using ProcessFunction = OSStatus (AUHostProcessor::*)(
        AudioUnitRenderActionFlags*,
        const AudioTimeStamp*,
        UInt32,
        UInt32,
        AudioBufferList*) noexcept;
    static constexpr const char* kNoProcessFunctionDiagnostic =
        "AU process function is not connected";

    AUHostProcessor() : m_auInstance(nullptr), m_bypassed(false), m_state(LoadState::Unloaded) {
        m_abl = reinterpret_cast<AudioBufferList*>(m_ablStorage);
    }

    ~AUHostProcessor() {
        shutdown();
    }

    void shutdown() {
        m_shuttingDown.store(true, std::memory_order_release);
        m_stateTransition.store(true, std::memory_order_release);
        m_processFunction.store(nullptr, std::memory_order_release);
        waitForQuiescence();
        closeNativeEditor(0);
        if (m_auInstance) {
            AudioUnitUninitialize(m_auInstance);
            AudioComponentInstanceDispose(m_auInstance);
            m_auInstance = nullptr;
        }
        m_state.store(LoadState::Unloaded, std::memory_order_release);
    }

    bool loadPlugin(OSType type, OSType subtype, OSType manufacturer) {
        shutdown();
        AudioComponentDescription desc = { type, subtype, manufacturer, 0, 0 };
        AudioComponent component = AudioComponentFindNext(nullptr, &desc);
        if (!component) {
            m_lastError.store(componentNotFoundError, std::memory_order_relaxed);
            m_state.store(LoadState::Failed, std::memory_order_release);
            return false;
        }

        OSStatus status = AudioComponentInstanceNew(component, &m_auInstance);
        if (status != noErr) {
            m_lastError.store(status, std::memory_order_relaxed);
            m_state.store(LoadState::Failed, std::memory_order_release);
            return false;
        }
        // Creating an AudioUnit instance is not the same as initializing it.
        // prepareToPlay() is the point at which it becomes operational.
        m_state.store(LoadState::InstanceCreated, std::memory_order_release);
        return true;
    }

    // GUI discovery is deliberately separate from audio readiness.  An AU
    // can render correctly while exposing no Cocoa editor, and callers must
    // never treat a successful audio instance as proof that a native view can
    // be embedded.
    bool hasNativeEditor() const noexcept override {
#if defined(__APPLE__)
        if (!m_auInstance) return false;
        UInt32 dataSize = 0;
        Boolean writable = false;
        return AudioUnitGetPropertyInfo(
                   m_auInstance, kAudioUnitProperty_CocoaUI,
                   kAudioUnitScope_Global, 0, &dataSize, &writable) == noErr &&
               dataSize >= sizeof(AudioUnitCocoaViewInfo);
#else
        return false;
#endif
    }

    uint64_t openNativeEditor(uintptr_t parent) noexcept override {
#if defined(__APPLE__)
        if (!m_auInstance || parent == 0 || m_nativeEditorView) return 0;
        UInt32 propertySize = 0;
        Boolean writable = false;
        if (AudioUnitGetPropertyInfo(m_auInstance, kAudioUnitProperty_CocoaUI,
                                     kAudioUnitScope_Global, 0, &propertySize, &writable) != noErr ||
            propertySize < sizeof(AudioUnitCocoaViewInfo)) return 0;
        AudioUnitCocoaViewInfo info{};
        UInt32 actualSize = sizeof(info);
        if (AudioUnitGetProperty(m_auInstance, kAudioUnitProperty_CocoaUI,
                                 kAudioUnitScope_Global, 0, &info, &actualSize) != noErr ||
            !info.mCocoaViewBundleLocation || !info.mCocoaViewClass) return 0;
        m_cocoaBundle = CFBundleCreate(nullptr, info.mCocoaViewBundleLocation);
        if (!m_cocoaBundle || !CFBundleLoadExecutable(m_cocoaBundle)) {
            if (m_cocoaBundle) { CFRelease(m_cocoaBundle); m_cocoaBundle = nullptr; }
            return 0;
        }
        char className[256]{};
        if (!CFStringGetCString(info.mCocoaViewClass, className, sizeof(className), kCFStringEncodingUTF8)) {
            closeNativeEditor(0);
            return 0;
        }
        Class factoryClass = objc_getClass(className);
        if (!factoryClass) { closeNativeEditor(0); return 0; }
        using SendId = id (*)(id, SEL);
        using SendAUView = id (*)(id, SEL, AudioUnit);
        id factory = reinterpret_cast<SendId>(objc_msgSend)(reinterpret_cast<id>(factoryClass), sel_registerName("alloc"));
        factory = factory ? reinterpret_cast<SendId>(objc_msgSend)(factory, sel_registerName("init")) : nil;
        id view = factory ? reinterpret_cast<SendAUView>(objc_msgSend)(factory, sel_registerName("uiViewForAudioUnit:"), m_auInstance) : nil;
        if (!view) {
            if (factory) reinterpret_cast<SendId>(objc_msgSend)(factory, sel_registerName("release"));
            closeNativeEditor(0);
            return 0;
        }
        using SendSubview = void (*)(id, SEL, id);
        reinterpret_cast<SendSubview>(objc_msgSend)(reinterpret_cast<id>(parent), sel_registerName("addSubview:"), view);
        m_nativeEditorFactory = factory;
        m_nativeEditorView = view;
        return reinterpret_cast<uint64_t>(view);
#else
        (void)parent;
        return 0;
#endif
    }

    bool closeNativeEditor(uint64_t session) noexcept override {
#if defined(__APPLE__)
        if (!m_nativeEditorView || (session != 0 && session != reinterpret_cast<uint64_t>(m_nativeEditorView))) return false;
        using SendVoid = void (*)(id, SEL);
        reinterpret_cast<SendVoid>(objc_msgSend)(reinterpret_cast<id>(m_nativeEditorView), sel_registerName("removeFromSuperview"));
        reinterpret_cast<SendVoid>(objc_msgSend)(reinterpret_cast<id>(m_nativeEditorView), sel_registerName("release"));
        if (m_nativeEditorFactory) reinterpret_cast<SendVoid>(objc_msgSend)(reinterpret_cast<id>(m_nativeEditorFactory), sel_registerName("release"));
        m_nativeEditorView = nullptr;
        m_nativeEditorFactory = nullptr;
        if (m_cocoaBundle) { CFBundleUnloadExecutable(m_cocoaBundle); CFRelease(m_cocoaBundle); m_cocoaBundle = nullptr; }
        return true;
#else
        (void)session;
        return false;
#endif
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_processFunction.store(nullptr, std::memory_order_release);
        if (!std::isfinite(sr) || sr <= 0.0 || bs == 0 || bs > kMaxBlockSize) {
            m_lastError.store(kInvalidAudioConfigurationError, std::memory_order_relaxed);
            m_state.store(LoadState::Failed, std::memory_order_release);
            return;
        }
        if (!m_auInstance) {
            m_state.store(LoadState::Failed, std::memory_order_release);
            return;
        }

        m_sampleRate = sr;
        m_maxBlockSize = bs;

        AudioStreamBasicDescription asbd{};
        asbd.mSampleRate = sr;
        asbd.mFormatID = kAudioFormatLinearPCM;
        asbd.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
        asbd.mBytesPerPacket = sizeof(float);
        asbd.mFramesPerPacket = 1;
        asbd.mBytesPerFrame = sizeof(float);
        asbd.mChannelsPerFrame = m_numChannels.load();
        asbd.mBitsPerChannel = 32;

        OSStatus err = AudioUnitSetProperty(m_auInstance, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &asbd, sizeof(asbd));
        if (err != noErr) { m_lastError.store(err, std::memory_order_relaxed); m_state.store(LoadState::Failed, std::memory_order_release); return; }

        err = AudioUnitSetProperty(m_auInstance, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &asbd, sizeof(asbd));
        if (err != noErr) { m_lastError.store(err, std::memory_order_relaxed); m_state.store(LoadState::Failed, std::memory_order_release); return; }

        err = AudioUnitSetProperty(m_auInstance, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &bs, sizeof(bs));
        if (err != noErr) { m_lastError.store(err, std::memory_order_relaxed); m_state.store(LoadState::Failed, std::memory_order_release); return; }

        err = AudioUnitInitialize(m_auInstance);
        if (err != noErr) { m_lastError.store(err, std::memory_order_relaxed); m_state.store(LoadState::Failed, std::memory_order_release); return; }
        m_processFunction.store(&AUHostProcessor::renderAudioUnit, std::memory_order_release);
        if (!hasProcessFunction()) {
            m_state.store(LoadState::Failed, std::memory_order_release);
            return;
        }
        m_lastError.store(noErr, std::memory_order_relaxed);
        m_shuttingDown.store(false, std::memory_order_release);
        m_stateTransition.store(false, std::memory_order_release);
        m_state.store(LoadState::Operational, std::memory_order_release);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        m_activeProcesses.fetch_add(1, std::memory_order_acq_rel);
        struct ProcessGuard {
            std::atomic<uint32_t>& count;
            ~ProcessGuard() { count.fetch_sub(1, std::memory_order_release); }
        } guard{m_activeProcesses};
        if (m_shuttingDown.load(std::memory_order_acquire)
            || m_stateTransition.load(std::memory_order_acquire)) {
            buffer.clear();
            return;
        }
        const ProcessFunction processFunction = m_processFunction.load(std::memory_order_acquire);
        const LoadState state = m_state.load(std::memory_order_acquire);
        if (state != LoadState::Operational || processFunction == nullptr || m_bypassed.load(std::memory_order_relaxed)) {
            if (state == LoadState::Operational && processFunction == nullptr) {
                m_processFailed.store(true, std::memory_order_release);
            }
            buffer.clear();
            return;
        }

        const uint32_t activeChannels = std::min((uint32_t)buffer.getNumChannels(), m_numChannels.load());
        const uint32_t numSamples = buffer.getNumSamples();
        if (buffer.getNumChannels() == 0 || buffer.getNumChannels() > MaxChannels ||
            activeChannels == 0 || numSamples == 0 || numSamples > m_maxBlockSize ||
            numSamples > kMaxBlockSize) {
            m_processFailed.store(true, std::memory_order_release);
            buffer.clear();
            return;
        }

        // 1. LOCK-FREE PARAMETER AUTOMATION SCHEDULING
        ParameterCmd cmd;
        while (m_paramQueue.pop(cmd)) {
            // AudioUnitSetParameter is a control API and is not guaranteed to
            // be realtime-safe. ScheduleParameters is explicitly marked
            // CA_REALTIME_API and applies the event to this render call.
            AudioUnitParameterEvent event{};
            event.scope = kAudioUnitScope_Global;
            event.element = 0;
            event.parameter = static_cast<AudioUnitParameterID>(cmd.id);
            event.eventType = kParameterEvent_Immediate;
            event.eventValues.immediate.bufferOffset =
                std::min(cmd.offsetSamples, static_cast<uint32_t>(numSamples));
            event.eventValues.immediate.value = cmd.value;
            const OSStatus parameterStatus =
                AudioUnitScheduleParameters(m_auInstance, &event, 1);
            if (parameterStatus != noErr) {
                m_lastError.store(parameterStatus, std::memory_order_relaxed);
                m_parameterScheduleFailed.store(true, std::memory_order_release);
            }
        }

        // 2. SAMPLE-ACCURATE MIDI DISPATCH
        const auto* events = midi.getEvents();
        for (size_t i = 0; i < midi.size(); ++i) {
            const auto& ev = events[i];
            // AudioUnit MIDI dispatch accepts a legacy 3-byte message, not a
            // 128-bit UMP or a variable-length SysEx payload. Also reject
            // timestamps outside this render slice instead of truncating a
            // uint64_t offset into an unrelated host sample.
            const uint8_t status = ev.size > 0 ? ev.data[0] & 0xF0u : 0u;
            if (ev.size == 3 && ev.data[0] != 0xF0u && status != 0xF0u &&
                ev.sampleOffset < numSamples &&
                ev.sampleOffset <= static_cast<uint64_t>(UINT32_MAX)) {
                MusicDeviceMIDIEvent(m_auInstance, ev.data[0], ev.data[1], ev.data[2], (uint32_t)ev.sampleOffset);
            }
        }

        // 3. N-CHANNEL ABL CONFIGURATION
        m_abl->mNumberBuffers = activeChannels;
        for (uint32_t c = 0; c < activeChannels; ++c) {
            m_abl->mBuffers[c].mNumberChannels = 1;
            m_abl->mBuffers[c].mDataByteSize = numSamples * sizeof(float);
            m_abl->mBuffers[c].mData = buffer.getWritePointer(c);
        }

        AudioUnitRenderActionFlags flags = 0;
        AudioTimeStamp timeStamp{};
        timeStamp.mSampleTime = static_cast<Float64>(context.playhead);
        timeStamp.mFlags = kAudioTimeStampSampleTimeValid;

        OSStatus status = noErr;
        {
            auto start = std::chrono::high_resolution_clock::now();
            status = (this->*processFunction)(&flags, &timeStamp, 0, numSamples, m_abl);
            auto end = std::chrono::high_resolution_clock::now();

            auto duration = std::chrono::duration_cast<std::chrono::microseconds>(end - start).count();

            // Calculate dynamic threshold as 80% of the total block time budget in microseconds
            double blockDurationUs = (static_cast<double>(numSamples) / m_sampleRate) * 1000000.0;
            int64_t thresholdUs = static_cast<int64_t>(blockDurationUs * 0.8);
            thresholdUs = std::max(thresholdUs, static_cast<int64_t>(500)); // Minimum 500us limit

            if (duration > thresholdUs) {
                 ++m_watchdogOverruns;
                 if (m_watchdogOverruns >= kWatchdogOverrunLimit) {
                     m_bypassed.store(true, std::memory_order_release);
                     m_watchdogTripPending.store(true, std::memory_order_release);
                     // The AU may have written only part of the block before
                     // exceeding its budget. Never expose that mixed block to
                     // downstream processors; the next block is explicit dry
                     // bypass and this block is deterministic silence.
                     buffer.clear(0, numSamples);
                     return;
                 }
            } else {
                 m_watchdogOverruns = 0;
            }
        }

        if (__builtin_expect(status != noErr, 0)) {
            m_lastError.store(status, std::memory_order_relaxed);
            // Graceful cleanup: Zero the buffer to prevent DC offset/noise spikes from failed AU
            buffer.clear(0, numSamples);
        } else {
            // Treat AU output as untrusted just like CLAP/VST3 output.  A
            // single NaN can otherwise poison meters, denormal handling, and
            // every downstream bus in the graph.
            for (uint32_t channel = 0; channel < activeChannels; ++channel) {
                float* output = buffer.getWritePointer(channel);
                for (uint32_t frame = 0; frame < numSamples; ++frame) {
                    if (!std::isfinite(output[frame])) {
                        output[frame] = 0.0f;
                        m_nonFiniteSamples.fetch_add(1, std::memory_order_relaxed);
                    }
                }
            }
        }
    }

    void scheduleParameter(uint32_t id, float value, uint32_t offset = 0) {
        ParameterCmd cmd{id, value, offset};
        if (!m_paramQueue.push(cmd)) {
            // Never fall back to AudioUnitSetParameter here: this method may be
            // called by a control producer while the render thread is active.
            // Preserve realtime safety and surface queue loss to the control
            // thread instead of silently dropping automation.
            m_parameterScheduleFailed.store(true, std::memory_order_release);
            m_parameterQueueOverruns.fetch_add(1, std::memory_order_relaxed);
        }
    }

    void setNumChannels(uint32_t n) { m_numChannels.store(std::min(n, MaxChannels)); }

    void reset() noexcept override {
        if (!m_auInstance || m_shuttingDown.load(std::memory_order_acquire)) return;
        beginStateTransition();
        const OSStatus status = AudioUnitReset(m_auInstance, kAudioUnitScope_Global, 0);
        m_lastError.store(status, std::memory_order_relaxed);
        if (status != noErr) m_stateRestoreFailed.store(true, std::memory_order_release);
        endStateTransition();
    }

    void setBypassed(bool bypassed) {
        m_bypassed.store(bypassed, std::memory_order_relaxed);
        if (!bypassed) {
            m_watchdogOverruns = 0;
            m_watchdogTripPending.store(false, std::memory_order_release);
        }
    }

    bool takeWatchdogTrip() noexcept override {
        return m_watchdogTripPending.exchange(false, std::memory_order_acq_rel);
    }

    LoadState loadState() const noexcept {
        const LoadState state = m_state.load(std::memory_order_acquire);
        return state == LoadState::Operational && !hasProcessFunction()
            ? LoadState::Failed
            : state;
    }
    bool hasProcessFunction() const noexcept {
        return m_processFunction.load(std::memory_order_acquire) != nullptr;
    }
    bool isOperational() const noexcept {
        return loadState() == LoadState::Operational && hasProcessFunction();
    }
    bool takeParameterScheduleFailure() noexcept {
        return m_parameterScheduleFailed.exchange(false, std::memory_order_acq_rel);
    }
    uint64_t parameterQueueOverruns() const noexcept {
        return m_parameterQueueOverruns.load(std::memory_order_relaxed);
    }
    uint64_t nonFiniteSampleCount() const noexcept override {
        return m_nonFiniteSamples.load(std::memory_order_acquire);
    }
    const char* processDiagnostic() const noexcept {
        if (!hasProcessFunction()) return kNoProcessFunctionDiagnostic;
        return m_parameterScheduleFailed.load(std::memory_order_acquire)
            ? "AU parameter scheduling is unsupported or failed"
            : "AU process function is connected";
    }
    OSStatus lastError() const noexcept { return m_lastError.load(std::memory_order_relaxed); }

    std::vector<uint8_t> getState() const override {
        auto* self = const_cast<AUHostProcessor*>(this);
        if (!self->m_auInstance || self->m_shuttingDown.load(std::memory_order_acquire)) return {};
        self->beginStateTransition();

        UInt32 propertySize = 0;
        Boolean writable = false;
        OSStatus status = AudioUnitGetPropertyInfo(
            self->m_auInstance, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global,
            0, &propertySize, &writable);
        (void)writable;
        if (status != noErr || propertySize == 0) {
            self->m_lastError.store(status != noErr ? status : kStateUnavailableError,
                                    std::memory_order_relaxed);
            self->m_stateRestoreFailed.store(true, std::memory_order_release);
            self->endStateTransition();
            return {};
        }

        CFPropertyListRef propertyList = nullptr;
        UInt32 pointerSize = sizeof(propertyList);
        status = AudioUnitGetProperty(
            self->m_auInstance, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global,
            0, &propertyList, &pointerSize);
        if (status != noErr || !propertyList) {
            self->m_lastError.store(status != noErr ? status : kStateUnavailableError,
                                    std::memory_order_relaxed);
            self->m_stateRestoreFailed.store(true, std::memory_order_release);
            self->endStateTransition();
            return {};
        }

        CFErrorRef error = nullptr;
        CFDataRef encoded = CFPropertyListCreateData(
            kCFAllocatorDefault, propertyList, kCFPropertyListBinaryFormat_v1_0, 0, &error);
        if (error) CFRelease(error);
        CFRelease(propertyList);
        if (!encoded || CFDataGetLength(encoded) > static_cast<CFIndex>(kMaxStateBytes)) {
            if (encoded) CFRelease(encoded);
            self->m_lastError.store(kStateTooLargeError, std::memory_order_relaxed);
            self->m_stateRestoreFailed.store(true, std::memory_order_release);
            self->endStateTransition();
            return {};
        }

        const CFIndex length = CFDataGetLength(encoded);
        const auto* bytes = CFDataGetBytePtr(encoded);
        std::vector<uint8_t> result;
        if (length > 0 && bytes) result.assign(bytes, bytes + length);
        CFRelease(encoded);
        self->m_lastError.store(noErr, std::memory_order_relaxed);
        self->endStateTransition();
        return result;
    }

    bool setState(const std::vector<uint8_t>& data) override {
        return setFullState(data);
    }

    bool restoreStateChecked(const std::vector<uint8_t>& data) override {
        return setFullState(data);
    }

    bool setFullState(const std::vector<uint8_t>& data) {
        if (!m_auInstance) return false;
        if (data.size() > kMaxStateBytes) {
            m_lastError.store(kStateTooLargeError, std::memory_order_relaxed);
            m_stateRestoreFailed.store(true, std::memory_order_release);
            return false;
        }
        beginStateTransition();
        CFDataRef encoded = CFDataCreate(kCFAllocatorDefault, data.data(), (CFIndex)data.size());
        if (!encoded) {
            m_lastError.store(kStateAllocationError, std::memory_order_relaxed);
            m_stateRestoreFailed.store(true, std::memory_order_release);
            endStateTransition();
            return false;
        }
        CFErrorRef decodeError = nullptr;
        CFPropertyListRef propertyList = CFPropertyListCreateWithData(
            kCFAllocatorDefault, encoded, kCFPropertyListImmutable, nullptr, &decodeError);
        if (decodeError) CFRelease(decodeError);
        CFRelease(encoded);
        if (!propertyList) {
            m_lastError.store(kStateDecodeError, std::memory_order_relaxed);
            m_stateRestoreFailed.store(true, std::memory_order_release);
            endStateTransition();
            return false;
        }
        OSStatus status = AudioUnitSetProperty(
            m_auInstance, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0,
            &propertyList, sizeof(propertyList));
        CFRelease(propertyList);
        m_lastError.store(status, std::memory_order_relaxed);
        if (status != noErr) m_stateRestoreFailed.store(true, std::memory_order_release);
        endStateTransition();
        return status == noErr;
    }

    bool takeStateRestoreFailure() noexcept {
        return m_stateRestoreFailed.exchange(false, std::memory_order_acq_rel);
    }

