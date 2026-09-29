    if (format == 0) {
        pcmStream = std::make_unique<::Hirari::IO::Persistence::WavWriter::Pcm16StreamWriter>(
            outputPath, totalSamples, outputRate);
        if (!pcmStream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_OPEN_FAILED");
            return false;
        }
    } else if (format == 1) {
        wave64Stream = std::make_unique<::Hirari::IO::Persistence::WavWriter::Wave64FloatStreamWriter>(
            outputPath, totalSamples, outputRate);
        if (!wave64Stream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_WAVE64_STREAM_OPEN_FAILED");
            return false;
        }
    } else if (format == 2) {
        floatStream = std::make_unique<::Hirari::IO::Persistence::WavWriter::Float32StreamWriter>(
            outputPath, outputRate);
        if (!floatStream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_FLOAT32_STREAM_OPEN_FAILED");
            return false;
        }
    } else {
        pcm24Stream = std::make_unique<::Hirari::IO::Persistence::WavWriter::Pcm24StreamWriter>(
            outputPath, totalSamples, outputRate);
        if (!pcm24Stream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_PCM24_STREAM_OPEN_FAILED");
            return false;
        }
    }
    AudioBuffer blockBuffer(2, 1024);
    ::Hirari::DSP::Utils::TPDFDither ditherLeft;
    ::Hirari::DSP::Utils::TPDFDither ditherRight;

    uint64_t originalPlayhead = m_playhead.load();
    bool wasPlaying = m_isPlaying.load();
    // The render path temporarily drives the live transport.  Every return
    // path below must restore the caller's transport state, including an
    // allocation failure or an exception from a plug-in/render callback.
    struct OfflinePlaybackStateGuard final {
        std::atomic<uint64_t>* playhead;
        std::atomic<bool>* playing;
        uint64_t originalPlayhead;
        bool originalPlaying;
        ~OfflinePlaybackStateGuard() noexcept {
            playhead->store(originalPlayhead, std::memory_order_release);
            playing->store(originalPlaying, std::memory_order_release);
        }
    } playbackStateGuard{&m_playhead, &m_isPlaying, originalPlayhead, wasPlaying};

    m_playhead.store(rangeActive ? rangeStart : 0);
    m_isPlaying.store(true);
    // Offline renders must be reproducible. Clear the master limiter's
    // look-ahead history so a previous playback/render cannot contaminate the
    // first blocks of this bounce.
    if (m_masterLimiter) m_masterLimiter->reset();
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (track) track->resetForOfflineRender();
        }
    }

    for (uint32_t s = 0; s < totalSamples; s += 1024) {
        if (!waitForBounceControl()) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        uint32_t sz = std::min(1024u, totalSamples - s);
        processBlock(blockBuffer, 0, sz);
        // Silence is valid output from the actual project graph. Never
        // replace it with direct track rendering: that would bypass solo,
        // routing, sends, and bus processing, and advance stateful processors
        // a second time after processBlock already rendered them.
        if (format == 0 || format == 3) {
            float* left = blockBuffer.getWritePointer(0);
            float* right = blockBuffer.getWritePointer(1);
            const float depthScale = format == 0 ? 256.0f : 1.0f;
            for (uint32_t sample = 0; sample < sz; ++sample) {
                left[sample] += ditherLeft.process() * depthScale;
                right[sample] += ditherRight.process() * depthScale;
            }
        }
        bool blockWritten = false;
        if (format == 0) {
            blockWritten = pcmStream->writeFrames(blockBuffer.getReadPointer(0),
                                                  blockBuffer.getReadPointer(1), sz);
        } else if (format == 1) {
            const float* channels[2] = {
                blockBuffer.getReadPointer(0), blockBuffer.getReadPointer(1)};
            blockWritten = wave64Stream->writeFrames(channels, sz);
        } else if (format == 2) {
            const float* channels[2] = {
                blockBuffer.getReadPointer(0), blockBuffer.getReadPointer(1)};
            blockWritten = floatStream->writeFrames(channels, sz);
        } else {
            blockWritten = pcm24Stream->writeFrames(blockBuffer.getReadPointer(0),
                                                    blockBuffer.getReadPointer(1), sz);
        }
        if (!blockWritten) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_WRITE_FAILED");
            return false;
        }
        m_bounceProgress.store(
            static_cast<float>(s + sz) / static_cast<float>(totalSamples),
            std::memory_order_release);
    }

    m_playhead.store(originalPlayhead);
    m_isPlaying.store(wasPlaying);

    // Each streaming writer participates in the same cancellation boundary
    // and publishes only a complete, synced file.
    {
        bool success = false;
        {
            std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
            if (!m_bounceCancelRequested.load(std::memory_order_acquire)) {
                if (format == 0) {
                    success = pcmStream->finish();
                } else if (format == 1) {
                    success = wave64Stream->finish();
                } else if (format == 2) {
                    success = floatStream->framesWritten() == totalSamples &&
                              floatStream->finish();
                } else {
                    success = pcm24Stream->finish();
                }
            }
        }
        if (!success) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            const bool cancelled = m_bounceCancelRequested.load(std::memory_order_acquire);
            m_bounceState.store(static_cast<uint32_t>(cancelled ? BounceState::Cancelled : BounceState::Failed),
                                std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0,
                cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_STREAM_PUBLISH_FAILED");
            return false;
        }
        finishOfflineRender();
        m_bounceProgress.store(1.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Completed), std::memory_order_release);
        return true;
    }
}

bool HirariUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    if (format > 3 || path.empty()) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        return false;
    }
    std::lock_guard<std::mutex> bounceLifecycleLock(m_bounceLifecycleMutex);
    if (m_bounceRunning.load(std::memory_order_acquire)) {
        const uint32_t state = m_bounceState.load(std::memory_order_acquire);
        const bool terminal = state == static_cast<uint32_t>(BounceState::Completed) ||
                              state == static_cast<uint32_t>(BounceState::Failed) ||
                              state == static_cast<uint32_t>(BounceState::Cancelled);
        if (!terminal) return false;
        // A terminal state is published just before the worker wrapper clears
        // m_bounceRunning. Join here so a retry cannot be rejected during
        // that narrow retirement window.
        if (m_bounceThread.joinable()) m_bounceThread.join();
        m_bounceRunning.store(false, std::memory_order_release);
    } else if (m_bounceThread.joinable()) {
        m_bounceThread.join();
    }
    m_bounceRunning.store(true, std::memory_order_release);
    m_bounceCancelRequested.store(false, std::memory_order_release);
    m_bouncePauseRequested.store(false, std::memory_order_release);
    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Queued), std::memory_order_release);
    const std::string outputPath(path);
    m_bounceThread = std::thread([this, outputPath, format]() {
        const bool success = bounce_project(outputPath, format);
        const bool cancelled = get_bounce_state() == static_cast<uint32_t>(BounceState::Cancelled);
        ::Hirari::Core::Diagnostics::LogBuffer::post(
            success ? 2 : (cancelled ? 1 : 0), 0,
            success ? "BOUNCE_COMPLETE" : (cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_FAILED"));
        m_bounceRunning.store(false, std::memory_order_release);
    });
    return true;
}

float HirariUnifiedEngine::get_bounce_progress() const noexcept {
    return std::clamp(m_bounceProgress.load(std::memory_order_acquire), 0.0f, 1.0f);
}

uint32_t HirariUnifiedEngine::get_bounce_state() const noexcept {
    return m_bounceState.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::cancel_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Queued) &&
        state != static_cast<uint32_t>(BounceState::Rendering) &&
        state != static_cast<uint32_t>(BounceState::Paused)) {
        return false;
    }
    {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        m_bounceCancelRequested.store(true, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }
    return true;
}

bool HirariUnifiedEngine::pause_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Rendering)) return false;
    m_bouncePauseRequested.store(true, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::resume_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Paused)) return false;
    m_bouncePauseRequested.store(false, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    if (root < 0 || root > 11 || type < 0 || type > 32) return false;
    m_rootNote.store(root, std::memory_order_relaxed);
    m_scaleType.store(type, std::memory_order_relaxed);
    return true;
}

bool HirariUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t mapHash) {
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    bool found = false;
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        if (state.activeTracks[i] && state.activeTracks[i]->getId() == trackId) {
            found = true;
            break;
        }
    }
    if (!found) return false;
    m_articulationMapHash.store(mapHash, std::memory_order_relaxed);
    return true;
}

bool HirariUnifiedEngine::set_expression_map(uint32_t trackId,
                                             const std::vector<uint32_t>& packed,
                                             const std::string& json,
                                             uint32_t articulationId) {
    if (packed.size() % 6 != 0 || packed.size() > 24480 || json.size() > 1024 * 1024 ||
        articulationId > 255) return false;
    std::array<uint8_t, 256> counts{};
    for (size_t index = 0; index < packed.size(); index += 6) {
        const uint32_t articulationId = packed[index];
        const uint32_t channel = packed[index + 1];
        const uint32_t kind = packed[index + 2];
        const uint32_t a = packed[index + 3];
        const uint32_t b = packed[index + 4];
        const uint32_t c = packed[index + 5];
        if (articulationId == 0 || articulationId >= counts.size() || channel >= 16 ||
            kind < 1 || kind > 16 || counts[articulationId] >= 16) return false;
        const uint32_t baseKind = kind > 5 && kind < 11 ? kind - 5 :
            kind > 10 && kind < 16 ? kind - 10 : kind;
        const bool validOutput = kind == 16 ? a >= 1 && a <= 16 && b == 0 && c == 0 :
            baseKind == 1 ? a < 128 && b >= 1 && b <= 127 && c <= 96000 :
            baseKind == 2 ? a < 128 && (b < 128 || b == 255) && (c < 128 || c == 255) :
            baseKind == 3 ? a < 128 && b < 128 :
            baseKind == 4 ? a < 128 :
            baseKind == 5 ? a < 128 && b < 128 : false;
        if (!validOutput) return false;
        ++counts[articulationId];
    }
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        auto* track = state.activeTracks[i];
        if (!track || track->getId() != trackId) continue;
        std::vector<uint32_t> beforePacked;
        track->getArticulationOutputMap(beforePacked);
        const std::string beforeJson = track->getExpressionMapJson();
        if (beforePacked == packed && beforeJson == json) return true;
        const auto applySnapshot = [this, trackId](const std::vector<uint32_t>& values,
                                                   const std::string& serialized) {
            const auto& current = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t index = 0; index < current.trackCount && index < 256; ++index) {
                auto* target = current.activeTracks[index];
                if (!target || target->getId() != trackId) continue;
                std::array<std::array<std::array<uint32_t, 5>, 16>, 256> snapshotOutputs{};
                std::array<uint8_t, 256> outputCounts{};
                for (size_t offset = 0; offset < values.size(); offset += 6) {
                    const auto id = static_cast<uint8_t>(values[offset]);
                    const auto slot = outputCounts[id]++;
                    snapshotOutputs[id][slot] = {
                        values[offset + 2], values[offset + 1], values[offset + 3],
                        values[offset + 4], values[offset + 5],
                    };
                }
                for (uint16_t rawId = 1; rawId < 256; ++rawId) {
                    const auto id = static_cast<uint8_t>(rawId);
                    if (!target->beginArticulationMapUpdate(id)) return;
                    for (uint8_t slot = 0; slot < outputCounts[rawId]; ++slot) {
                        const auto& output = snapshotOutputs[rawId][slot];
                        if (!target->setArticulationOutput(id, slot,
                                static_cast<uint8_t>(output[0]), static_cast<uint8_t>(output[1]),
                                static_cast<uint8_t>(output[2]), static_cast<uint8_t>(output[3]),
                                static_cast<uint8_t>(output[4]))) return;
                    }
                    if (!target->finishArticulationMapUpdate(id, outputCounts[rawId])) return;
                }
                target->setExpressionMapJson(serialized);
                return;
            }
        };
        applySnapshot(packed, json);
        const std::string actionName = "Edit MIDI Expression Map T" + std::to_string(trackId) +
            (articulationId == 0 ? std::string{} : " A" + std::to_string(articulationId));
        m_undoManager.recordAppliedAction(actionName,
            [applySnapshot, beforePacked, beforeJson] { applySnapshot(beforePacked, beforeJson); },
            [applySnapshot, afterPacked = packed, afterJson = json] { applySnapshot(afterPacked, afterJson); },
            true);
        return true;
    }
    return false;
}

bool HirariUnifiedEngine::set_expression_map_pro(uint32_t trackId, const std::string& json) {
    if (trackId == 0 || json.size() > 1024 * 1024) return false;
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        auto* track = state.activeTracks[i];
        if (!track || track->getId() != trackId) continue;
        const std::string before = track->getExpressionMapProJson();
        if (before == json) return true;
        const auto applySnapshot = [this, trackId](const std::string& snapshot) {
            const auto& current = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t index = 0; index < current.trackCount && index < 256; ++index) {
                auto* target = current.activeTracks[index];
                if (target && target->getId() == trackId) {
                    target->setExpressionMapProJson(snapshot);
                    return;
                }
            }
        };
        track->setExpressionMapProJson(json);
        m_undoManager.recordAppliedAction(
            "Edit MIDI Expression Map Pro T" + std::to_string(trackId),
            [applySnapshot, before] { applySnapshot(before); },
            [applySnapshot, after = json] { applySnapshot(after); }, true);
        return true;
    }
    return false;
}
