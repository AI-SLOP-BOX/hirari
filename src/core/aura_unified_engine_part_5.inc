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
    }

    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Rendering), std::memory_order_release);

    const uint64_t owner = aura_callback_thread_token();
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

    const uint64_t fallbackLength = static_cast<uint64_t>(outputRate) * 30u;
    uint64_t boundedLength = projectEnd > 0 ? projectEnd : fallbackLength;
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
    const uint32_t totalSamples = static_cast<uint32_t>(std::min<uint64_t>(boundedLength, UINT32_MAX));
    const std::string outputPath(path);
    // PCM16 is the default product bounce format. Stream it directly to the
    // durable writer so long projects do not allocate two full-length float
    // buffers before the first byte reaches disk. WAVE64 remains on the
    // existing bounded-buffer path until a float32 streaming writer is added.
    std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter> pcmStream;
    std::unique_ptr<AudioBuffer> bounceL;
    std::unique_ptr<AudioBuffer> bounceR;
    if (format == 0) {
        pcmStream = std::make_unique<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter>(
            outputPath, totalSamples, outputRate);
        if (!pcmStream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_OPEN_FAILED");
            return false;
        }
    } else {
        bounceL = std::make_unique<AudioBuffer>(1, totalSamples);
        bounceR = std::make_unique<AudioBuffer>(1, totalSamples);
    }
    AudioBuffer blockBuffer(2, 1024);
    
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
    
    m_playhead.store(0); 
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
        if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        uint32_t sz = std::min(1024u, totalSamples - s);
        processBlock(blockBuffer, 0, sz);
        if (format == 0) {
            if (!pcmStream->writeFrames(blockBuffer.getReadPointer(0),
                                        blockBuffer.getReadPointer(1), sz)) {
                m_playhead.store(originalPlayhead);
                m_isPlaying.store(wasPlaying);
                finishOfflineRender();
                m_bounceProgress.store(0.0f, std::memory_order_release);
                m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
                ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_WRITE_FAILED");
                return false;
            }
        } else {
            std::memcpy(bounceL->getWritePointer(0) + s, blockBuffer.getReadPointer(0), sz * sizeof(float));
            std::memcpy(bounceR->getWritePointer(0) + s, blockBuffer.getReadPointer(1), sz * sizeof(float));
        }
        m_bounceProgress.store(
            static_cast<float>(s + sz) / static_cast<float>(totalSamples),
            std::memory_order_release);
    }
    
    m_playhead.store(originalPlayhead); 
    m_isPlaying.store(wasPlaying);
    
    // The streaming writer owns PCM16 publication. It still participates in
    // the same cancellation boundary as the buffered WAVE64 path.
    if (format == 0) {
        bool success = false;
        {
            std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
            if (!m_bounceCancelRequested.load(std::memory_order_acquire)) {
                success = pcmStream->finish();
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
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0,
                cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_STREAM_PUBLISH_FAILED");
            return false;
        }
        finishOfflineRender();
        m_bounceProgress.store(1.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Completed), std::memory_order_release);
        return true;
    }

    // A thread hash alone is not a publication identity: a later render in
    // another process (or a recycled worker thread) can produce the same
    // value.  Include a process-local monotonic token so a stale renderer can
    // never accidentally target a newer temporary pathname.
    static std::atomic<uint64_t> renderPublicationSequence{0};
    const uint64_t publicationSequence = renderPublicationSequence.fetch_add(
        1, std::memory_order_relaxed) + 1;
    const std::string temporaryOutputPath = outputPath + ".tmp-render-" +
        std::to_string(owner) + "-" + std::to_string(publicationSequence);
    // Cancellation can arrive after the last audio block but before the
    // temporary file is published.  Never let a cancelled render replace a
    // newer result at the destination.
    if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
        m_playhead.store(originalPlayhead);
        m_isPlaying.store(wasPlaying);
        finishOfflineRender();
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
        return false;
    }
    bool success = format == 1
        ? ::Aura::Core::Utils::WavWriter::writeWave64(
            temporaryOutputPath, bounceL->getReadPointer(0), bounceR->getReadPointer(0), totalSamples, outputRate)
        : ::Aura::Core::Utils::WavWriter::write(
            temporaryOutputPath, bounceL->getReadPointer(0), bounceR->getReadPointer(0), totalSamples, outputRate);
    if (success) {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
            success = false;
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
        } else {
            std::error_code publishError;
            std::filesystem::rename(temporaryOutputPath, outputPath, publishError);
            success = !publishError;
        }
    }
    if (!success) {
        std::error_code cleanupError;
        std::filesystem::remove(temporaryOutputPath, cleanupError);
    }
    if (!success) {
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_FAILED");
    }
    finishOfflineRender();
    m_bounceProgress.store(success ? 1.0f : 0.0f, std::memory_order_release);
    const bool cancelled = m_bounceCancelRequested.load(std::memory_order_acquire);
    m_bounceState.store(
        static_cast<uint32_t>(cancelled ? BounceState::Cancelled
                                        : (success ? BounceState::Completed : BounceState::Failed)),
        std::memory_order_release);
    return success;
}

bool AuraUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    if (format > 1 || path.empty()) {
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
    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Queued), std::memory_order_release);
    const std::string outputPath(path);
    m_bounceThread = std::thread([this, outputPath, format]() {
        const bool success = bounce_project(outputPath, format);
        const bool cancelled = get_bounce_state() == static_cast<uint32_t>(BounceState::Cancelled);
        ::Aura::Core::Diagnostics::LogBuffer::post(
            success ? 2 : (cancelled ? 1 : 0), 0,
            success ? "BOUNCE_COMPLETE" : (cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_FAILED"));
        m_bounceRunning.store(false, std::memory_order_release);
    });
    return true;
}

float AuraUnifiedEngine::get_bounce_progress() const noexcept {
    return std::clamp(m_bounceProgress.load(std::memory_order_acquire), 0.0f, 1.0f);
}

uint32_t AuraUnifiedEngine::get_bounce_state() const noexcept {
    return m_bounceState.load(std::memory_order_acquire);
}

bool AuraUnifiedEngine::cancel_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Queued) &&
        state != static_cast<uint32_t>(BounceState::Rendering)) {
        return false;
    }
    {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        m_bounceCancelRequested.store(true, std::memory_order_release);
    }
    return true;
}

bool AuraUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    if (root < 0 || root > 11 || type < 0 || type > 32) return false;
    m_rootNote.store(root, std::memory_order_relaxed);
    m_scaleType.store(type, std::memory_order_relaxed);
    return true;
}

bool AuraUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t mapHash) {
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
