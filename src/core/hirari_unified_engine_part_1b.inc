        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || !track->canProcess(sz)) continue;
            const auto& work = track->getWorkBuffer(sz);
            routing.captureFeedback(track->getId(), work.getReadPointer(0),
                                    work.getReadPointer(1), sz);
        }

        // Publish per-track post-FX peaks through the lock-free telemetry
        // double buffer.  The UI expects linear amplitudes and adds the
        // master channel separately from the master meter, so keep this
        // payload strictly aligned with the active track order.
        {
            while (m_telemetryCopyLock.test_and_set(std::memory_order_acquire)) {
            }
            m_telemetrySequence.fetch_add(1, std::memory_order_acq_rel);
            const uint32_t activeTelemetry =
                m_activeTelemetryIdx.load(std::memory_order_relaxed);
            const uint32_t nextTelemetry = activeTelemetry ^ 1u;
            auto& telemetry = m_telemetryBuffers[nextTelemetry];
            telemetry.count = std::min(state.trackCount, kMaxTracks);
            std::array<uint32_t, kMaxTracks> telemetryTrackIds{};
            std::array<const float*, kMaxTracks> trackPeaksLeft{};
            std::array<const float*, kMaxTracks> trackPeaksRight{};
            for (uint32_t i = 0; i < telemetry.count; ++i) {
                Track* track = state.activeTracks[i];
                if (track && track->canProcess(sz)) {
                    telemetryTrackIds[i] = track->getId();
                    const auto& work = track->getWorkBuffer(sz);
                    trackPeaksLeft[i] = work.getReadPointer(0);
                    trackPeaksRight[i] = work.getReadPointer(
                        work.getNumChannels() > 1 ? 1u : 0u);
                }
            }
            hirari_mixer_track_peaks(
                m_mixerTelemetry.nativeState(), telemetryTrackIds.data(),
                trackPeaksLeft.data(), trackPeaksRight.data(), telemetry.count, sz,
                telemetry.peaksL, telemetry.peaksR);
            telemetry.version++;
            m_activeTelemetryIdx.store(nextTelemetry, std::memory_order_release);
            m_telemetrySequence.fetch_add(1, std::memory_order_release);
            m_telemetryCopyLock.clear(std::memory_order_release);
        }

        // The metronome is part of the rendered monitor signal, not a track
        // node.  Inject it after graph/routing processing so it remains
        // sample-accurate even when tracks are muted or an offline target is
        // selected, and let the master limiter handle the final headroom.
        float* clickLeft = m_metronomeClickLeft.data();
        float* clickRight = m_metronomeClickRight.data();
        std::fill_n(clickLeft, sz, 0.0f);
        std::fill_n(clickRight, sz, 0.0f);
        const bool renderMetronome =
            m_isPlaying.load(std::memory_order_relaxed) && m_metronome.isEnabled();
        if (renderMetronome) {
            const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
            const double bpm = m_tempoMap.currentBpm();
            m_metronome.process(clickLeft, clickRight, sz, currentPlayhead, sampleRate, bpm);
        }

        float* dest_l = output.getWritePointer(0, off);
        float* dest_r = output.getWritePointer(1, off);
        const float masterGain = m_masterGain.load(std::memory_order_acquire);
        hirari_master_output_process(
            dest_l, dest_r, clickLeft, clickRight, sz, masterGain, renderMetronome);

        // Build the callback context locally. The control thread may update
        // the persistent preparation context during device reconfiguration;
        // the audio callback must never observe a partially updated struct.
        ::Hirari::DSP::ProcessContext callbackContext{};
        callbackContext.playhead = currentPlayhead;
        callbackContext.blockStart = currentPlayhead;
                callbackContext.blockEnd = currentPlayhead <=
                    std::numeric_limits<uint64_t>::max() - sz
                    ? currentPlayhead + sz
                    : std::numeric_limits<uint64_t>::max();
        callbackContext.sampleRate = std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
        callbackContext.bpm = m_tempoMap.currentBpm();
        callbackContext.blockSize = sz;
        callbackContext.audioConfigGeneration = callbackGeneration;
        callbackContext.isPlaying = m_isPlaying.load(std::memory_order_acquire);
        callbackContext.anyoneSoloed = anySolo;
        callbackContext.isLooping = m_loopEnabled.load(std::memory_order_acquire);
        callbackContext.cycleStart = cycleStart;
        callbackContext.cycleEnd = cycleEnd;
        // The master limiter is part of the rendered signal as well as the
        // live monitor path. Offline bounces now append the configured tail,
        // so its look-ahead history is flushed instead of being truncated at
        // the content boundary.
        if (m_masterLimiter) {
            // Limit only the active [offset, offset+size) window. Passing the
            // owning output buffer here would re-process stale samples when
            // processBlockDirect() renders into a non-zero offset.
            float* masterChannels[2] = {dest_l, dest_r};
            AudioBuffer masterView;
            masterView.wrapChannels(masterChannels, 2, sz);
            m_masterLimiter->process(masterView, m_blockMidi, callbackContext);
        }

        // The public meter API measures the post-limiter master before
        // monitor-only speaker trim and DIM are applied.
        if (m_analyzer) {
            m_analyzer->process(output.getReadPointer(0, off),
                                output.getReadPointer(1, off), sz);
        }

        // Control Room is strictly device-monitoring. Never bake speaker trim
        // or DIM into an offline master/stem render.
        if (!m_offlineRenderActive.load(std::memory_order_acquire)) {
            const uint32_t activeCueBus = m_controlRoom.activeCueMixBusTrackId();
            uint32_t cueOutputChannel = m_controlRoom.activeCueMixOutputChannel();
            const bool cueUsesDedicatedPair = cueOutputChannel >= 2;
            const bool cueOutputAvailable = cueOutputChannel + 1 < output.getNumChannels();
            const bool cueClickEnabled = m_controlRoom.activeCueMixClickEnabled();
            if (!cueOutputAvailable) cueOutputChannel = 0;
            if (activeCueBus != 0 && (!cueUsesDedicatedPair || cueOutputAvailable)) {
                auto* cueBus = dynamic_cast<BusTrack*>(findTrack(activeCueBus));
                if (cueBus && cueBus->canProcess(sz)) {
                    const auto& cue = cueBus->getWorkBuffer(sz);
                    const float* cueLeft = cue.getReadPointer(0);
                    const float* cueRight = cue.getReadPointer(1);
                    float* monitorLeft = output.getWritePointer(cueOutputChannel, off);
                    float* monitorRight = output.getWritePointer(cueOutputChannel + 1, off);
                    const float cueGain = m_controlRoom.activeCueMixGain();
                    hirari_control_room_mix_cue(
                        cueLeft, cueRight, m_metronomeClickLeft.data(), m_metronomeClickRight.data(),
                        monitorLeft, monitorRight, sz, cueGain, cueClickEnabled);
                }
            }
            const float* talkback = nullptr;
            if (m_talkbackInputPublished.load(std::memory_order_acquire)) {
                const uint32_t publishedFrames =
                    m_talkbackInputFrames.load(std::memory_order_acquire);
                const uint32_t inputOffset = m_talkbackInputOffset;
                if (inputOffset < publishedFrames && publishedFrames - inputOffset >= sz) {
                    talkback = m_talkbackInput.data() + inputOffset;
                }
            }
            if (cueUsesDedicatedPair) {
                // Keep the Main control-room monitor live while a selected
                // Cue feeds its own output pair.  Talkback is kept on that
                // Cue pair so it does not leak into the control-room mix.
                m_controlRoom.processMonitor(output.getWritePointer(0, off),
                                              output.getWritePointer(1, off), sz);
                if (cueOutputAvailable) {
                    m_controlRoom.processMonitorWithTalkback(
                        output.getWritePointer(cueOutputChannel, off),
                        output.getWritePointer(cueOutputChannel + 1, off), talkback, sz);
                }
            } else {
                m_controlRoom.processMonitorWithTalkback(
                    output.getWritePointer(cueOutputChannel, off),
                    output.getWritePointer(cueOutputChannel + 1, off), talkback, sz);
            }
        }


        // 3. System Telemetry & Adaptive Sovereignty (CPU Governor)
        float currentLoad = m_dspLoad.load(std::memory_order_relaxed);
        // Diagnostics are useful, but formatting/publishing on every block
        // needlessly competes with the audio callback. Keep a coarse heartbeat
        // and leave the detailed telemetry to the control-side poller.
        if (sampleCpu) {
            m_engineDiagnostics.recordRealtimeBlock();
        }

        // INDUSTRIAL: Adaptive CPU Governor
        // If the system is nearing its limits (>95%), we forensic-ly throttle background tracks.
        if (currentLoad > 0.95f) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                Track* track = state.activeTracks[i];
                if (track && (track->getType() == Track::Audio || track->getType() == Track::Instrument)) {
                    track->setEcoMode(true);
                }
            }
            m_ecoModeLogTriggered.store(true, std::memory_order_release);
        } else if (currentLoad < 0.70f) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                Track* track = state.activeTracks[i];
                if (track) {
                    track->setEcoMode(false);
                }
            }
        }

        // --- AUTOMATION RECORDING ---
        if (m_isPlaying.load(std::memory_order_acquire) &&
            m_autoRecordMode.load(std::memory_order_relaxed) > 0) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                if (state.activeTracks[i] && state.activeTracks[i]->isRecordArmed()) {
                    state.activeTracks[i]->recordCurrentState(currentPlayhead);
                }
            }
        }

        // (NaN/Inf sanitizer has already run before the limiter above)
        if (m_isPlaying.load(std::memory_order_relaxed)) {
            // INDUSTRIAL: Real-time Tonal Sync (Scale Quantize)
            if (m_isAscended.load(std::memory_order_relaxed)) {
                // ...
            }

            // INDUSTRIAL: Frame-Accurate Video Sync
            if (m_showVideo.load(std::memory_order_relaxed)) {
                // In a production bridge, we would call the Rust Orchestrator here.
                m_videoSyncLogTriggered.store(true, std::memory_order_release);
            }

            if (currentPlayhead <= std::numeric_limits<uint64_t>::max() - sz) {
                m_playhead.fetch_add(sz, std::memory_order_relaxed);
            } else {
                m_audioRangeOverflow.store(true, std::memory_order_release);
            }
        }
        if (sampleCpu) {
            const auto elapsed = std::chrono::duration<float>(
                std::chrono::steady_clock::now() - cpuStart).count();
            const float blockSeconds = static_cast<float>(sz) /
                static_cast<float>(std::max(1.0, m_sampleRate.load(std::memory_order_relaxed)));
            const float load = blockSeconds > 0.0f ? elapsed / blockSeconds : 0.0f;
            m_dspLoad.store(std::clamp(load, 0.0f, 4.0f), std::memory_order_relaxed);
        }
    }

void HirariUnifiedEngine::processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples) {
    m_audioCallbacksInFlight.fetch_add(1, std::memory_order_acquire);
    struct CallbackGuard {
        std::atomic<uint32_t>& count;
        ~CallbackGuard() { count.fetch_sub(1, std::memory_order_release); }
    } callbackGuard{m_audioCallbacksInFlight};

    const bool validInput = buffers != nullptr && numChannels >= 2 && numChannels <= 32 &&
        numSamples > 0 && numSamples <= HirariUnifiedEngine::kMaxAudioBlockSize &&
        buffers[0] != nullptr && buffers[1] != nullptr;
    if (!validInput) {
        // The caller does not provide the actual allocation length.  When the
        // declared block size is invalid, writing even a bounded prefix could
        // overrun a shorter external buffer.  Fail closed without touching
        // memory whose extent cannot be proven at this boundary.
        m_talkbackInputPublished.store(false, std::memory_order_release);
        m_talkbackInputFrames.store(0, std::memory_order_release);
        return;
    }
    for (uint32_t channel = 0; channel < numChannels; ++channel) {
        if (buffers[channel] == nullptr) return;
    }
    if (m_isShutdown.load(std::memory_order_acquire)) {
        AudioBuffer silence;
        silence.wrapChannels(buffers, numChannels, numSamples);
        silence.clear(numSamples);
        m_talkbackInputPublished.store(false, std::memory_order_release);
        m_talkbackInputFrames.store(0, std::memory_order_release);
        return;
    }
    // Zero-Allocation Wrapper (Allocated on stack for absolute concurrency safety).
    // Track work buffers and the sandbox protocol are prepared for bounded
    // engine blocks.  A host/device callback may legally be larger than that
    // bound, so feed the same external buffer in bounded slices rather than
    // silently skipping every track whose prepared buffer is smaller.
    AudioBuffer wrap;
    wrap.wrapChannels(buffers, numChannels, numSamples);
    constexpr uint32_t kEngineBlockSize = 4096;
    for (uint32_t offset = 0; offset < numSamples;) {
        const uint32_t remaining = numSamples - offset;
        uint32_t chunk = std::min(kEngineBlockSize, remaining);

        // A hardware callback may straddle the cycle end.  Process the tail
        // of the current cycle and the head of the next cycle as separate
        // engine chunks so notes, automation and click timing never observe
        // a playhead that runs past the loop boundary.
        if (m_isPlaying.load(std::memory_order_acquire) &&
            m_loopEnabled.load(std::memory_order_acquire)) {
            const uint64_t cycleStart = m_cycleStart.load(std::memory_order_acquire);
            const uint64_t cycleEnd = m_cycleEnd.load(std::memory_order_acquire);
            const uint64_t playhead = m_playhead.load(std::memory_order_relaxed);
            if (cycleEnd > cycleStart) {
                if (playhead >= cycleEnd) {
                    m_playhead.store(cycleStart, std::memory_order_relaxed);
                    continue;
                }
                const uint64_t untilCycleEnd = cycleEnd - playhead;
                if (untilCycleEnd < chunk) {
                    chunk = static_cast<uint32_t>(untilCycleEnd);
                }
            }
        }
        if (chunk == 0) continue;
        m_inputMonitorBlockOffset = offset;
        m_talkbackInputOffset = offset;
        processBlock(wrap, offset, chunk);
        offset += chunk;
    }
    m_inputMonitorBlockPublished.store(false, std::memory_order_release);
    m_talkbackInputPublished.store(false, std::memory_order_release);
    m_talkbackInputFrames.store(0, std::memory_order_release);
}
