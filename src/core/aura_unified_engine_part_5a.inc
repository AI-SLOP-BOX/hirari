uint64_t AuraUnifiedEngine::get_video_frame_revision() const noexcept {
#if defined(__APPLE__)
    return ::Aura::Core::Engine::VideoBridge::frameRevision();
#else
    return 0;
#endif
}

bool AuraUnifiedEngine::request_video_frame(double seconds) {
    if (!std::isfinite(seconds) || seconds < 0.0) return false;
#if defined(__APPLE__)
    return ::Aura::Core::Engine::VideoBridge::requestFrameAt(seconds);
#else
    (void)seconds;
    return false;
#endif
}

bool AuraUnifiedEngine::load_video(std::string_view path) {
    if (path.empty()) return false;
#if defined(__APPLE__)
    std::string owned(path);
    return ::Aura::Core::Engine::VideoBridge::loadVideo(owned.c_str());
#else
    (void)path;
    return false;
#endif
}

rust::Vec<float> AuraUnifiedEngine::get_region_waveform(uint32_t tid, uint32_t rid, uint32_t numPeaks) {
    rust::Vec<float> peaks;

    if (numPeaks == 0) return peaks;
    
    // 1. ATOMIC CACHE PROBE (Zero-Wait, Zero-Copy mmap Access)
    {
        std::lock_guard<std::mutex> lock(m_cacheMutex);
        auto it = m_peakCache.find(rid);
        if (it != m_peakCache.end() && it->second.valid.load(std::memory_order_acquire)) {
            const auto& cache = it->second;
            if (cache.mappedData && cache.mappedSize > 0) {
                // Adaptive Downsampling from High-Res Columnar Memory-Mapped Cache
                uint32_t cacheSize = (uint32_t)(cache.mappedSize / sizeof(float)) / 2;
                for (uint32_t i = 0; i < numPeaks; ++i) {
                    uint32_t srcIdx = (i * cacheSize / numPeaks) * 2;
                    peaks.push_back(cache.mappedData[srcIdx]);
                    peaks.push_back(cache.mappedData[srcIdx + 1]);
                }
                return peaks;
            }
        }
    }

    // 2. Decode the source file and build the cache from real samples.
    std::string regionPath;
    {
        std::lock_guard<std::mutex> trackLock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    regionPath = region.path;
                    break;
                }
            }
            if (!regionPath.empty()) break;
        }
    }

    if (regionPath.empty()) return peaks;
    auto audioData = m_audioDecoderManager.importFile(regionPath);
    if (!audioData || audioData->getNumChannels() == 0 || audioData->getNumSamples() == 0) {
        return peaks;
    }

    // Safe control-thread fallback.  The mmap/cache builder below is not part
    // of the audio path, but waveform requests must never crash the UI when a
    // scheduler or cache generation is unavailable.  Generate the requested
    // envelope directly from the decoded source and return it immediately.
    // This also gives callers useful waveform data on the first request.
    const uint32_t sampleCount = audioData->getNumSamples();
    const uint32_t channels = audioData->getNumChannels();
    const float* left = audioData->getReadPointer(0);
    const float* right = channels > 1 ? audioData->getReadPointer(1) : left;
    if (left == nullptr || right == nullptr || sampleCount == 0) return peaks;
    peaks.reserve(static_cast<size_t>(numPeaks) * 2u);
    for (uint32_t i = 0; i < numPeaks; ++i) {
        const uint32_t begin = static_cast<uint32_t>(
            (static_cast<uint64_t>(i) * sampleCount) / numPeaks);
        const uint32_t end = std::max(
            begin + 1u,
            static_cast<uint32_t>((static_cast<uint64_t>(i + 1u) * sampleCount) / numPeaks));
        float maxPeak = 0.0f;
        float minPeak = 0.0f;
        for (uint32_t sample = begin; sample < std::min(end, sampleCount); ++sample) {
            const float l = left[sample];
            const float r = right[sample];
            if (std::isfinite(l)) {
                maxPeak = std::max(maxPeak, l);
                minPeak = std::min(minPeak, l);
            }
            if (std::isfinite(r)) {
                maxPeak = std::max(maxPeak, r);
                minPeak = std::min(minPeak, r);
            }
        }
        peaks.push_back(maxPeak);
        peaks.push_back(minPeak);
    }
    const uint64_t buildGeneration = m_waveformGeneration.load(std::memory_order_acquire);
    {
        std::lock_guard<std::mutex> lock(m_cacheMutex);
        // The UI polls while a background build is running. Keep one build
        // per region generation so a stale worker cannot block a new project.
        auto [it, inserted] = m_peakBuilds.emplace(rid, buildGeneration);
        if (!inserted) {
            if (it->second == buildGeneration) return peaks;
            it->second = buildGeneration;
        }
    }

    // Columnar peak database write and mmap allocation.
    const bool queued = m_threadPool.postTaskAsync(0, [this, tid, rid, audioData, buildGeneration]() {
        const auto finishBuild = [this, rid, buildGeneration]() {
            std::lock_guard<std::mutex> lock(m_cacheMutex);
            auto it = m_peakBuilds.find(rid);
            if (it != m_peakBuilds.end() && it->second == buildGeneration) {
                m_peakBuilds.erase(it);
            }
        };
        const uint32_t highResCount = 2048; // Standard high-res target
        const uint32_t numFloats = highResCount * 2;

        std::vector<float> peaksBuffer;
        peaksBuffer.reserve(numFloats);
        const uint32_t sampleCount = audioData->getNumSamples();
        const uint32_t channels = audioData->getNumChannels();
        const uint32_t bucketCount = std::min(highResCount, sampleCount);
        if (bucketCount == 0 || channels == 0) {
            finishBuild();
            return;
        }

        const float* left = audioData->getReadPointer(0);
        const float* right = channels > 1 ? audioData->getReadPointer(1) : left;
        for (uint32_t i = 0; i < highResCount; ++i) {
            const uint32_t begin = static_cast<uint32_t>((static_cast<uint64_t>(i) * sampleCount) / highResCount);
            const uint32_t end = std::max(begin + 1, static_cast<uint32_t>((static_cast<uint64_t>(i + 1) * sampleCount) / highResCount));
            float maxPeak = 0.0f;
            float minPeak = 0.0f;
            for (uint32_t sample = begin; sample < std::min(end, sampleCount); ++sample) {
                const float l = left[sample];
                const float r = right[sample];
                if (std::isfinite(l)) { maxPeak = std::max(maxPeak, l); minPeak = std::min(minPeak, l); }
                if (std::isfinite(r)) { maxPeak = std::max(maxPeak, r); minPeak = std::min(minPeak, r); }
            }
            peaksBuffer.push_back(maxPeak);
            peaksBuffer.push_back(minPeak);
        }

        if (m_waveformGeneration.load(std::memory_order_acquire) != buildGeneration) {
            finishBuild();
            return;
        }
        
        // Safety validation: verify that the region still exists before storing peaks
        bool exists = false;
        {
            std::lock_guard<std::mutex> trackLock(m_tracksMutex);
            for (const auto& t : m_tracks) {
                if (!t || t->getId() != tid) continue;
                for (const auto& r : t->getRegions()) {
                    if (r.id == rid) { exists = true; break; }
                }
                if (exists) break;
            }
        }
        
        if (exists) {
            std::error_code fsError;
            const auto tempRoot = std::filesystem::temp_directory_path(fsError);
            if (fsError) {
                finishBuild();
                return;
            }
            const std::filesystem::path cacheDir = tempRoot / "aura-studio" / "waveform_cache";
            std::filesystem::create_directories(cacheDir, fsError);
            if (fsError) {
                finishBuild();
                return;
            }
            std::filesystem::path cachePath = cacheDir /
                ("peaks_cache_" + std::to_string(rid) + "_" +
                 std::to_string(buildGeneration) + ".bin");
            std::string filename = cachePath.string();

            ScopedFd file(::open(filename.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0600));
            if (file.fd >= 0) {
                size_t bytesToWrite = numFloats * sizeof(float);
                bool writeComplete = ::ftruncate(file.fd, static_cast<off_t>(bytesToWrite)) == 0;
                size_t offset = 0;
                while (writeComplete && offset < bytesToWrite) {
                    const ssize_t written = ::write(file.fd, peaksBuffer.data() + offset / sizeof(float),
                                                    bytesToWrite - offset);
                    if (written < 0 && errno == EINTR) continue;
                    if (written <= 0) { writeComplete = false; break; }
                    offset += static_cast<size_t>(written);
                }

                if (writeComplete && offset == bytesToWrite && ::fsync(file.fd) == 0) {
                    // Map the columnar database file directly into virtual memory
                    ScopedMapping mapping(::mmap(nullptr, bytesToWrite, PROT_READ, MAP_SHARED, file.fd, 0), bytesToWrite);
                    if (mapping.data != MAP_FAILED) {
                        std::lock_guard<std::mutex> lock(m_cacheMutex);
                        if (m_waveformGeneration.load(std::memory_order_acquire) != buildGeneration) {
                            auto staleBuild = m_peakBuilds.find(rid);
                            if (staleBuild != m_peakBuilds.end() &&
                                staleBuild->second == buildGeneration) {
                                m_peakBuilds.erase(staleBuild);
                            }
                            return;
                        }
                        auto& cache = m_peakCache[rid];
                        
                        // Clean up existing memory mapping if any
                        cache.cleanup();
                        
                        cache.mappedData = static_cast<float*>(mapping.release());
                        cache.mappedSize = bytesToWrite;
                        cache.fd = file.release();
                        cache.sampleCount = numFloats;
                        cache.filepath = filename;
                        cache.valid.store(true, std::memory_order_release);
                        auto buildIt = m_peakBuilds.find(rid);
                        if (buildIt != m_peakBuilds.end() &&
                            buildIt->second == buildGeneration) {
                            m_peakBuilds.erase(buildIt);
                        }
                        return; // Successfully mapped and stored
                    }
                }
            }
        }
        finishBuild();
    });

    if (!queued) {
        std::lock_guard<std::mutex> lock(m_cacheMutex);
        auto it = m_peakBuilds.find(rid);
        if (it != m_peakBuilds.end() && it->second == buildGeneration) {
            m_peakBuilds.erase(it);
        }
    }

    return peaks; // Initially empty, will populate on next UI poll
}

rust::Vec<float> AuraUnifiedEngine::get_region_audio_samples(
    uint32_t tid, uint32_t rid, uint32_t maxSamples) {
    rust::Vec<float> samples;
    if (maxSamples == 0 || maxSamples > 16u * 1024u * 1024u) return samples;
    std::string regionPath;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) { regionPath = region.path; break; }
            }
            break;
        }
    }
    if (regionPath.empty()) return samples;
    auto audio = m_audioDecoderManager.importFile(regionPath);
    if (!audio || audio->getNumChannels() == 0 || audio->getNumSamples() == 0) return samples;
    // Never return a partial region: callers write the returned buffer back as
    // a replacement, so truncating here would silently destroy the tail.
    if (audio->getNumSamples() > maxSamples) return samples;
    const uint32_t count = audio->getNumSamples();
    const float* left = audio->getReadPointer(0);
    if (!left || count == 0) return samples;
    samples.reserve(count);
    for (uint32_t i = 0; i < count; ++i) {
        samples.push_back(std::isfinite(left[i]) ? left[i] : 0.0f);
    }
    return samples;
}

rust::Vec<float> AuraUnifiedEngine::get_region_audio_interleaved(
    uint32_t tid, uint32_t rid, uint32_t maxFrames) {
    rust::Vec<float> samples;
    if (maxFrames == 0 || maxFrames > 16u * 1024u * 1024u) return samples;
    std::string regionPath;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) { regionPath = region.path; break; }
            }
            break;
        }
    }
    if (regionPath.empty()) return samples;
    auto audio = m_audioDecoderManager.importFile(regionPath);
    if (!audio || audio->getNumChannels() != 2 || audio->getNumSamples() > maxFrames) return samples;
    const float* left = audio->getReadPointer(0);
    const float* right = audio->getReadPointer(1);
    if (!left || !right || audio->getNumSamples() == 0) return samples;
    samples.reserve(static_cast<size_t>(audio->getNumSamples()) * 2u);
    for (uint32_t i = 0; i < audio->getNumSamples(); ++i) {
        samples.push_back(std::isfinite(left[i]) ? left[i] : 0.0f);
        samples.push_back(std::isfinite(right[i]) ? right[i] : 0.0f);
    }
    return samples;
}

double AuraUnifiedEngine::get_region_sample_rate(uint32_t tid, uint32_t rid) {
    if (tid == 0 || rid == 0) return 0.0;
    std::string regionPath;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) { regionPath = region.path; break; }
            }
            break;
        }
    }
    if (regionPath.empty()) return 0.0;
    // AudioBuffer intentionally carries no source-rate metadata, so ask the
    // decoder manager for the rate captured during the same import operation.
    auto audio = m_audioDecoderManager.importFile(regionPath);
    if (!audio) return 0.0;
    const double rate = m_audioDecoderManager.getLastSampleRate();
    return std::isfinite(rate) && rate > 0.0 ? rate : 0.0;
}

uint32_t AuraUnifiedEngine::get_region_channel_count(uint32_t tid, uint32_t rid) {
    std::string regionPath;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) { regionPath = region.path; break; }
            }
            break;
        }
    }
    if (regionPath.empty()) return 0;
    auto audio = m_audioDecoderManager.importFile(regionPath);
    return audio ? audio->getNumChannels() : 0;
}

void AuraUnifiedEngine::browser_preview(std::string_view path) {
    // Browser actions run on the control/UI side, never from processBlock.
    // Decode before publishing the immutable snapshot so the audio callback
    // only performs bounded sample reads and interpolation.
    if (path.empty()) {
        clear_preview_sample();
        return;
    }

    ::Aura::Core::IO::WavDecoder decoder;
    const std::string filePath(path);
    if (!decoder.open(filePath)) {
        clear_preview_sample();
        return;
    }

    AudioBuffer decoded;
    decoder.decodeFull(decoded);
    if (decoded.getNumChannels() == 0 || decoded.getNumSamples() == 0) {
        clear_preview_sample();
        return;
    }

    const uint32_t channels = decoded.getNumChannels();
    const uint32_t frames = decoded.getNumSamples();
    std::vector<float> mono(frames, 0.0f);
    for (uint32_t frame = 0; frame < frames; ++frame) {
        float sum = 0.0f;
        uint32_t validChannels = 0;
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float* source = decoded.getReadPointer(channel);
            if (source && std::isfinite(source[frame])) {
                sum += source[frame];
                ++validChannels;
            }
        }
        mono[frame] = validChannels == 0
            ? 0.0f
            : std::clamp(sum / static_cast<float>(validChannels), -1.0f, 1.0f);
    }
    const double sampleRate = decoder.getSampleRate();
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) {
        clear_preview_sample();
        return;
    }
    set_preview_sample(mono.data(), mono.size(), sampleRate);
    trigger_preview_sample();
}


bool AuraUnifiedEngine::bounce_project(std::string_view path, uint32_t format) {
    // Format 0 is RIFF/RF64; format 1 explicitly selects WAVE64. Reject
    // unsupported values instead of silently producing a different container.
    const auto extension = path.rfind('.');
    const auto asciiLower = [](char c) {
        return c >= 'A' && c <= 'Z' ? static_cast<char>(c + ('a' - 'A')) : c;
    };
    const auto suffix = extension == std::string_view::npos
        ? std::string_view{}
        : path.substr(extension + 1);
    const bool wavOutput = (suffix.size() == 3
        && asciiLower(suffix[0]) == 'w'
        && asciiLower(suffix[1]) == 'a'
        && asciiLower(suffix[2]) == 'v')
        || (suffix.size() == 4
        && asciiLower(suffix[0]) == 'r'
        && asciiLower(suffix[1]) == 'f'
        && asciiLower(suffix[2]) == '6'
        && asciiLower(suffix[3]) == '4');
    if (format > 1 || path.empty() || !wavOutput) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        ::Aura::Core::Diagnostics::LogBuffer::post(1, 0, "BOUNCE_UNSUPPORTED_FORMAT_OR_PATH");
        return false;
    }

    // Project loads and deferred structural edits publish tracks through the
    // control-side container first.  Rebuild the immutable callback snapshot
    // before an offline render so a bounce can never observe an empty/stale
    // track graph (which otherwise produces a valid but silent WAV).
    syncStructuralChanges();

    // Async callers clear this before publishing Queued so a cancellation
    // requested while queued is preserved. Direct synchronous callers need a
    // fresh request flag for each invocation.
    if (!m_bounceRunning.load(std::memory_order_acquire)) {
        m_bounceCancelRequested.store(false, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }

    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Rendering), std::memory_order_release);

    const uint64_t owner = aura_callback_thread_token();
    const bool previousOfflineRenderThread = aura_offline_render_thread;
    aura_offline_render_thread = true;
    struct OfflineRenderThreadGuard final {
        bool previous;
        ~OfflineRenderThreadGuard() { aura_offline_render_thread = previous; }
    } offlineRenderThreadGuard{previousOfflineRenderThread};
    bool expected = false;
    const auto offlineAcquireDeadline = std::chrono::steady_clock::now() +
        std::chrono::seconds(5);
    while (!m_offlineRenderActive.compare_exchange_weak(
        expected, true, std::memory_order_acq_rel, std::memory_order_acquire)) {
        expected = false;
        if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        // Never leave the render worker spinning forever behind a stale
        // callback/offline-render owner. A bounded failure is recoverable;
        // an immortal QUEUED render is not.
        if (std::chrono::steady_clock::now() >= offlineAcquireDeadline) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_OFFLINE_RENDER_BUSY_TIMEOUT");
            return false;
        }
        std::this_thread::yield();
    }
    m_offlineRenderOwner.store(owner, std::memory_order_release);

    const auto finishOfflineRender = [this]() {
        m_offlineRenderOwner.store(0, std::memory_order_release);
        m_offlineRenderActive.store(false, std::memory_order_release);
    };
    // Every path after acquiring the offline-render ownership must release it,
    // including allocation/IO failures and future early returns.  Keep this
    // guard deliberately tiny: it only publishes the ownership transition and
    // does not touch render state or the audio graph.
    struct OfflineRenderGuard final {
        std::atomic<uint64_t>* owner;
        std::atomic<bool>* active;
        ~OfflineRenderGuard() {
            owner->store(0, std::memory_order_release);
            active->store(false, std::memory_order_release);
        }
    } offlineRenderGuard{&m_offlineRenderOwner, &m_offlineRenderActive};

    // Pause is an offline-render control-plane operation.  It is deliberately
    // checked between blocks so the audio callback is never blocked and the
    // last completed block remains a valid, deterministic checkpoint.
    const auto waitForBounceControl = [this]() -> bool {
        for (;;) {
            if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
                return false;
            }
            if (!m_bouncePauseRequested.load(std::memory_order_acquire)) {
                m_bounceState.store(static_cast<uint32_t>(BounceState::Rendering),
                                    std::memory_order_release);
                return true;
            }
            m_bounceState.store(static_cast<uint32_t>(BounceState::Paused),
                                std::memory_order_release);
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    };

    const double configuredRate = m_sampleRate.load(std::memory_order_acquire);
    if (!std::isfinite(configuredRate) || configuredRate <= 0.0 || configuredRate > 384000.0) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        ::Aura::Core::Diagnostics::LogBuffer::post(1, 0, "BOUNCE_INVALID_SAMPLE_RATE");
        return false;
    }
    const uint32_t outputRate = static_cast<uint32_t>(configuredRate);
    uint64_t projectEnd = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            for (const auto& region : track->getRegions()) {
                const uint64_t loopCount = std::max<uint32_t>(region.loopCount, 1);
                const uint64_t timelineLength = region.len > UINT64_MAX / loopCount
                    ? UINT64_MAX
                    : region.len * loopCount;
                const uint64_t regionEnd = region.start > UINT64_MAX - timelineLength
                    ? UINT64_MAX
                    : region.start + timelineLength;
                projectEnd = std::max(projectEnd, regionEnd);
            }
        }
    }

    // MIDI-only projects do not have audio regions, but their scheduled notes
    // still define the render extent. Without this, bounce_project() falls
    // back to a fixed 30-second render and produces a long silent tail (and,
    // for an otherwise MIDI-only project, can look like a silent export).
    // Snapshot the active note list under the control-side mutex so a note
    // publication cannot race this offline render's extent calculation.
    {
        std::lock_guard<std::mutex> lock(m_midiNotesMutex);
        const uint32_t midiSlot = m_activeMidiSnapshot.load(std::memory_order_acquire);
        for (const auto& note : m_midiNoteSnapshots[midiSlot]) {
            const uint64_t noteEnd = note.startSample > UINT64_MAX - note.lengthSamples
                ? UINT64_MAX
                : note.startSample + note.lengthSamples;
            projectEnd = std::max(projectEnd, noteEnd);
        }
    }

    // Empty projects still need a valid, non-zero render for preview/export
    // callers, but a thirty-second silent fallback makes retries needlessly
    // expensive (especially for large track graphs).  Use a compact one
    // second bed; real projects are always bounded by their actual content
    // extent above.
    const uint64_t fallbackLength = static_cast<uint64_t>(outputRate);
    const bool rangeActive = m_offlineRenderRangeActive.load(std::memory_order_acquire);
    const uint64_t rangeStart = m_offlineRenderStartSample.load(std::memory_order_acquire);
    const uint64_t rangeEnd = m_offlineRenderEndSample.load(std::memory_order_acquire);
    if (rangeActive && rangeStart >= rangeEnd) {
        finishOfflineRender();
        return false;
    }
    uint64_t boundedLength = projectEnd > 0 ? projectEnd : fallbackLength;
    if (rangeActive) {
        boundedLength = rangeEnd - rangeStart;
    }
    // A render target may contain stateful processors (reverb, delay, vocal
    // tails) whose output continues after the last region. Keep this tail
    // explicit and bounded instead of silently truncating at projectEnd.
    const uint32_t tailMillis = m_offlineRenderTailMillis.load(std::memory_order_acquire);
    if (tailMillis > 0) {
        const uint64_t tail = (static_cast<uint64_t>(outputRate) * tailMillis) / 1000u;
        boundedLength = boundedLength > UINT64_MAX - tail
            ? UINT64_MAX
            : boundedLength + tail;
    }
    // The look-ahead limiter owns a small delay line. Always reserve that
    // latency at the end of a bounce, even when the user-selected effect tail
    // is zero, otherwise the final delayed samples disappear from the file.
    if (m_masterLimiter) {
        const uint64_t limiterTail = m_masterLimiter->getTailSamples();
        boundedLength = boundedLength > UINT64_MAX - limiterTail
            ? UINT64_MAX
            : boundedLength + limiterTail;
    }
    const uint32_t totalSamples = static_cast<uint32_t>(std::min<uint64_t>(boundedLength, UINT32_MAX));
    const std::string outputPath(path);
    // PCM16 is the default product bounce format. Stream it directly to the
    // durable writer so long projects do not allocate two full-length float
    // buffers before the first byte reaches disk. WAVE64 remains on the
    // existing bounded-buffer path until a float32 streaming writer is added.
    std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter> pcmStream;
