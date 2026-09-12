    bool process(AudioBuffer& audio, MidiBuffer& midi) noexcept {
        if (!isAlive()) return false;
        m_lastProcessOverrun.store(false, std::memory_order_release);
        m_processCalls.fetch_add(1, std::memory_order_acq_rel);
        struct ProcessGuard {
            std::atomic<uint32_t>& calls;
            ~ProcessGuard() { calls.fetch_sub(1, std::memory_order_release); }
        } guard{m_processCalls};
        // A state/reconfigure transaction may begin immediately after the
        // initial liveness check.  Count this call before checking the gate so
        // the control thread can wait for it deterministically; never submit
        // audio while state IPC is mutating the shared mailbox.
        if (m_stateTransition.load(std::memory_order_acquire) || !isAlive()) {
            audio.clear();
            midi.clear();
            return false;
        }
        // The shared mailbox has a fixed channel layout. Never silently
        // process only the first channels of a wider caller buffer: that
        // would produce plausible-looking but channel-corrupted audio for
        // surround or multichannel routes. A graph adapter must negotiate a
        // matching layout before this worker is used.
        if (m_shared == nullptr || audio.getNumChannels() == 0 ||
            audio.getNumChannels() > m_channels ||
            audio.getNumChannels() > SandboxProtocol::kMaxChannels ||
            audio.getNumSamples() == 0 || audio.getNumSamples() > SandboxProtocol::kMaxFrames) {
            audio.clear();
            midi.clear();
            return false;
        }

        const uint32_t currentChannels = std::min<uint32_t>(audio.getNumChannels(), SandboxProtocol::kMaxChannels);
        const uint32_t currentFrames = std::min<uint32_t>(audio.getNumSamples(), SandboxProtocol::kMaxFrames);
        const uint64_t completed = m_shared->completedSequence.load(std::memory_order_acquire);
        const bool requestInFlight =
            m_lastSubmitted != 0 &&
            m_shared->requestSequence.load(std::memory_order_acquire) != completed;
        // A completed mailbox block is copied back into the caller's buffer.
        // Preserve this call's input first so collecting the previous block
        // cannot accidentally feed the previous output back into the worker.
        size_t inputMidiCount = 0;
        uint32_t parameterChangeCount = 0;
        if (!requestInFlight) {
            for (uint32_t channel = 0; channel < currentChannels; ++channel) {
                const float* source = audio.getReadPointer(channel);
                if (!source) { audio.clear(); return false; }
                std::memcpy(m_pendingInput[channel], source,
                            static_cast<size_t>(currentFrames) * sizeof(float));
            }
            m_pendingChannels = currentChannels;
            m_pendingFrames = currentFrames;
            for (size_t sourceIndex = 0;
                 sourceIndex < midi.size() && inputMidiCount < SandboxProtocol::kMaxMidiEvents;
                 ++sourceIndex) {
                const auto& event = midi.getEvents()[sourceIndex];
                // The mailbox belongs to this exact render slice. Do not
                // forward a later event and rely on each plugin adapter to
                // clamp or discard it differently; doing so breaks sample
                // accuracy at loop and sub-block boundaries.
                if (event.sampleOffset >= currentFrames) {
                    m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
                    continue;
                }
                if (event.size > sizeof(SandboxProtocol::MidiEvent::data)) {
                    // Keep large SysEx/MIDI 2.0 payloads intact in the
                    // bounded extended ring instead of truncating them into
                    // the legacy event mailbox.
                    if (!m_shared->extendedMidi.push(event.sampleOffset,
                                                     event.articulationId,
                                                     event.data, event.size)) {
                        m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
                    }
                    continue;
                }
                auto& pending = m_pendingMidi[inputMidiCount++];
                pending.sampleOffset = event.sampleOffset;
                pending.size = event.size;
                pending.articulationId = event.articulationId;
                std::memcpy(pending.data, event.data, pending.size);
            }
            if (midi.size() > SandboxProtocol::kMaxMidiEvents) {
                m_shared->inputMidiTruncations.fetch_add(
                    static_cast<uint32_t>(midi.size() - SandboxProtocol::kMaxMidiEvents),
                    std::memory_order_relaxed);
            }
            const uint64_t parameterWrite = m_parameterWrite.load(std::memory_order_acquire);
            uint64_t parameterRead = m_parameterRead.load(std::memory_order_relaxed);
            const uint64_t available = parameterWrite - parameterRead;
            parameterChangeCount = static_cast<uint32_t>(std::min<uint64_t>(
                available, SandboxProtocol::kMaxParameterChanges));
            for (uint32_t index = 0; index < parameterChangeCount; ++index) {
                m_shared->parameterChange[index] = m_pendingParameterChanges[
                    (parameterRead + index) % SandboxProtocol::kMaxParameterChanges];
            }
            m_parameterRead.store(parameterRead + parameterChangeCount,
                                   std::memory_order_release);
        }
        bool produced = false;
        bool submitPreservedInput = false;
        // Audio mailbox ownership is one-block exact: accepting a later
        // sequence here would copy a newer block into the wrong caller.
        if (m_lastSubmitted != 0 && completed == m_lastSubmitted) {
            const uint32_t channels = std::min<uint32_t>(audio.getNumChannels(), SandboxProtocol::kMaxChannels);
            const uint32_t frames = std::min<uint32_t>(audio.getNumSamples(), SandboxProtocol::kMaxFrames);
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(audio.getWritePointer(channel), m_shared->output[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            const uint32_t outputMidiCount = std::min<uint32_t>(
                m_shared->outputMidiEvents.load(std::memory_order_acquire),
                SandboxProtocol::kMaxMidiEvents);
            for (uint32_t index = 0; index < outputMidiCount; ++index) {
                const auto& event = m_shared->outputMidi[index];
                if (event.sampleOffset < frames && event.size > 0 && event.size <= sizeof(event.data)) {
                    midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                }
            }
            // The plugin's output is appended after the input events. Sort
            // once at the boundary so downstream processors see monotonic
            // sample offsets without any allocation.
            midi.sort();
            produced = true;
            submitPreservedInput = true;
            m_completedSequenceHadOverrun = m_sequenceHadOverrun;
            m_sequenceHadOverrun = false;
            m_lastSubmitted = 0;
            m_pendingAgeFrames = 0;
            m_overrunReportedForSequence = false;
            if (m_shared->processErrors.exchange(0, std::memory_order_acq_rel) > 0) {
                m_failure.store(Failure::ProcessFailed, std::memory_order_release);
                audio.clear();
                return false;
            }
        }

        // Never overwrite an in-flight block. The caller receives a bounded
        // silent fallback for this cycle instead of blocking the audio thread.
        if (m_shared->requestSequence.load(std::memory_order_acquire) != completed) {
            // A caller may poll the one-block mailbox several times before
            // the worker is scheduled.  That is normal pending state, not an
            // overrun.  Only flag a deadline miss after the bounded grace
            // period; otherwise a healthy worker is quarantined by a tight
            // test/UI polling loop.
            // Audio-thread deadline accounting is expressed in frames rather
            // than wall-clock calls. Wall-clock hang detection remains on the
            // control thread in pollHealth().
            const double deadlineFrames = std::max(1.0, m_sampleRate * 0.005);
            const uint64_t ageIncrement = currentFrames;
            m_pendingAgeFrames = m_pendingAgeFrames >
                    std::numeric_limits<uint64_t>::max() - ageIncrement
                ? std::numeric_limits<uint64_t>::max()
                : m_pendingAgeFrames + ageIncrement;
            if (!m_overrunReportedForSequence &&
                static_cast<double>(m_pendingAgeFrames) >= deadlineFrames) {
                m_shared->mailboxOverruns.fetch_add(1, std::memory_order_relaxed);
                m_lastProcessOverrun.store(true, std::memory_order_release);
                m_sequenceHadOverrun = true;
                // Count one deadline miss per submitted sequence. Repeated
                // polling of the same in-flight block must not turn a single
                // slow plugin call into eight independent overruns and an
                // unjust quarantine.
                m_overrunReportedForSequence = true;
            }
            if (!produced) audio.clear();
            return produced;
        }

        const uint32_t channels = submitPreservedInput ? m_pendingChannels : currentChannels;
        const uint32_t frames = submitPreservedInput ? m_pendingFrames : currentFrames;
        m_shared->channels.store(channels, std::memory_order_relaxed);
        m_shared->frames.store(frames, std::memory_order_relaxed);
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float* source = submitPreservedInput ? m_pendingInput[channel] : audio.getReadPointer(channel);
            if (source == nullptr) { audio.clear(); return produced; }
            std::memcpy(m_shared->input[channel], source, static_cast<size_t>(frames) * sizeof(float));
        }
        for (size_t index = 0; index < inputMidiCount; ++index) {
            const auto& event = m_pendingMidi[index];
            auto& destination = m_shared->midi[index];
            destination.sampleOffset = event.sampleOffset;
            destination.size = event.size;
            destination.articulationId = event.articulationId;
            std::memcpy(destination.data, event.data, destination.size);
        }
        m_shared->midiEvents.store(static_cast<uint32_t>(inputMidiCount), std::memory_order_relaxed);
        m_shared->parameterChanges.store(parameterChangeCount, std::memory_order_release);
        m_shared->outputMidiEvents.store(0, std::memory_order_relaxed);
        m_shared->outputMidiDropped.store(0, std::memory_order_relaxed);
        const uint64_t sequence = ++m_sequence;
        m_lastSubmitted = sequence;
        m_pendingAgeFrames = 0;
        m_overrunReportedForSequence = false;
        m_shared->requestSequence.store(sequence, std::memory_order_release);
        if (!produced) audio.clear();
        return produced;
    }

