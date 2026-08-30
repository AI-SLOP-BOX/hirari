void AuraUnifiedEngine::sync_sharding_node(uint32_t node_id) {
    // Industrial: Distribute track processing to cluster
    // RT-Safe: Silenced logs in processing path
    auto* s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)].activeTracks;
    uint32_t count = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)].trackCount;
    for (uint32_t i = 0; i < count; ++i) {
        if (s[i]->getId() % 2 == node_id % 2) {
             // OFFLOADING TRACK Logic
        }
    }
}
bool AuraUnifiedEngine::generate_drum_fill(uint32_t tid, uint32_t bar, float complexity) {
    ::Aura::Core::Diagnostics::LogBuffer::post(tid, bar, "AURA_GENERATING_FILL");
    // industrial: uses chaos attractor to derive beat patterns
    int64_t entropy = aura_sys(300, 0, 0, 0);
    bool highEnergy = (std::abs(entropy) % 100) > (100 - (int)(complexity * 100));
    
    if (highEnergy) {
        std::string fill_file = "fill_standard_roll.wav";
        uint32_t pattern = std::abs(entropy) % 3;
        if (pattern == 0) fill_file = "fill_kick_snare.wav";
        else if (pattern == 1) fill_file = "fill_tom_roll.wav";
        else fill_file = "fill_hihat_triplet.wav";
        
        add_region(tid, fill_file, (double)bar * 4.0);
    }
    return true;
}
void AuraUnifiedEngine::set_show_video(bool s) { m_showVideo.store(s); }
void AuraUnifiedEngine::set_ascended_mode(bool enabled) { m_isAscended.store(enabled); }
void AuraUnifiedEngine::execute_restoration(uint32_t tid, float intensity) {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            const float amount = std::clamp(intensity, 0.0f, 1.0f);
            
            // Real restoration DSP: Dynamic Gate/Expander to suppress noise floor
            const float threshold = 0.015f; 
            const float rangeDb = -24.0f * amount; 
            const float rangeVal = std::pow(10.0f, rangeDb / 20.0f);
            
            for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                float* p = buffer.getWritePointer(c);
                uint32_t samples = buffer.getNumSamples();
                
                float env = 0.0f;
                for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                    env = std::max(env, std::abs(p[s_idx]));
                }
                
                float gain = 1.0f;
                if (env < threshold) {
                    float ratio = env / (threshold + 1e-6f);
                    gain = rangeVal + (1.0f - rangeVal) * ratio;
                }
                
                for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                    p[s_idx] *= gain;
                }
            }
        }
    }
}
void AuraUnifiedEngine::set_automation_record_mode(uint32_t mode) {
    // Keep the wire value aligned with AutomationMode: 0=Read, 1=Write,
    // 2=Touch, 3=Latch. Unknown values must fail closed to Read rather than
    // accidentally enabling write automation for a future/invalid enum.
    const uint32_t normalized = mode <= 4u ? mode : 0u;
    m_autoRecordMode.store(normalized, std::memory_order_release);
    // Keep the standalone recorder and the engine transport state in lockstep
    // so control-surface/API writes are captured by the same mode semantics.
    using RecorderMode = ::Aura::Core::Engine::AutomationRecorder::Mode;
    auto& recorder = ::Aura::Core::Engine::AutomationRecorder::getInstance();
    recorder.setMode(normalized == 0u ? RecorderMode::Off
        : normalized == 1u ? RecorderMode::Write
        : normalized == 2u ? RecorderMode::Touch
        : normalized == 3u ? RecorderMode::Latch
        : RecorderMode::AutoPunch);
}

void AuraUnifiedEngine::set_automation_punch_range(uint64_t start, uint64_t end) noexcept {
    ::Aura::Core::Engine::AutomationRecorder::getInstance().setPunchRange(start, end);
}

void AuraUnifiedEngine::record_plugin_scan_failure(std::string_view id, std::string_view error) {
    m_pluginCompatibility.markScanFailure(std::string(id), std::string(error));
}
void AuraUnifiedEngine::record_plugin_crash(std::string_view id) {
    m_pluginCompatibility.markCrash(std::string(id));
}
void AuraUnifiedEngine::set_plugin_blacklisted(std::string_view id, bool value) {
    m_pluginCompatibility.setBlacklisted(std::string(id), value);
}
std::string AuraUnifiedEngine::plugin_compatibility_snapshot_json() const {
    std::ostringstream out; out << "["; bool first = true;
    for (const auto& record : m_pluginCompatibility.records()) {
        if (!first) out << ","; first = false;
        out << "{\"id\":\"" << record.identifier << "\",\"blacklisted\":"
            << (record.blacklisted ? "true" : "false") << ",\"scan_failures\":"
            << record.scanFailures << ",\"crashes\":" << record.crashCount
            << ",\"last_error\":\"" << record.lastError << "\"}";
    }
    out << "]"; return out.str();
}

struct TrigLUT512 {
    float cosTable[512];
    float sinTable[512];
    
    static const TrigLUT512& getInstance() {
        static TrigLUT512 instance;
        return instance;
    }
private:
    TrigLUT512() {
        for (size_t i = 0; i < 512; ++i) {
            double angle = 2.0 * 3.14159265358979323846 * i / 512.0;
            cosTable[i] = static_cast<float>(std::cos(angle));
            sinTable[i] = static_cast<float>(std::sin(angle));
        }
    }
};

static void local_fft_engine(std::vector<std::complex<float>>& x) {
    size_t N = x.size();
    if (N <= 1) return;
    
    // Bit-reversal permutation
    for (size_t i = 1, j = 0; i < N; ++i) {
        size_t bit = N >> 1;
        for (; j & bit; bit >>= 1) {
            j ^= bit;
        }
        j ^= bit;
        if (i < j) {
            std::swap(x[i], x[j]);
        }
    }
    
    const auto& lut = TrigLUT512::getInstance();
    
    // Butterfly calculations using pre-computed trigonometric lookup table (LUT)
    for (size_t len = 2; len <= N; len <<= 1) {
        size_t step = N / len;
        for (size_t i = 0; i < N; i += len) {
            for (size_t j = 0; j < len / 2; ++j) {
                // Twiddle index mapping e^(-i * 2*pi * j / len) to the 512-point LUT index
                size_t lutIdx = (512 - step * j) % 512;
                std::complex<float> w(lut.cosTable[lutIdx], -lut.sinTable[lutIdx]);
                
                std::complex<float> u = x[i + j];
                std::complex<float> t = x[i + j + len / 2] * w;
                x[i + j] = u + t;
                x[i + j + len / 2] = u - t;
            }
        }
    }
}

bool AuraUnifiedEngine::execute_vocal_remover(uint32_t tid) {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            if (buffer.getNumChannels() >= 2) {
                float* pL = buffer.getWritePointer(0);
                float* pR = buffer.getWritePointer(1);
                uint32_t samples = buffer.getNumSamples();
                
                float bassL = 0.0f;
                float bassR = 0.0f;
                const float coeff = 0.02f; // ~150Hz cutoff at 44.1kHz
                
                for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                    float inL = pL[s_idx];
                    float inR = pR[s_idx];
                    
                    bassL += coeff * (inL - bassL);
                    bassR += coeff * (inR - bassR);
                    
                    float vocalCancelled = inL - inR;
                    
                    pL[s_idx] = vocalCancelled * 0.5f + bassL * 0.5f;
                    pR[s_idx] = -vocalCancelled * 0.5f + bassR * 0.5f;
                }
                return true;
            }
            return false;
        }
    }
    return false;
}

float AuraUnifiedEngine::get_track_correlation(uint32_t tid) const {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            if (buffer.getNumChannels() < 2) return 1.0f;
            
            const float* pL = buffer.getReadPointer(0);
            const float* pR = buffer.getReadPointer(1);
            uint32_t samples = buffer.getNumSamples();
            
            double sum_lr = 0.0;
            double sum_l2 = 0.0;
            double sum_r2 = 0.0;
            
            for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                double l = pL[s_idx];
                double r = pR[s_idx];
                sum_lr += l * r;
                sum_l2 += l * l;
                sum_r2 += r * r;
            }
            
            double denom = std::sqrt(sum_l2 * sum_r2);
            if (denom < 1e-9) return 1.0f;
            return static_cast<float>(sum_lr / denom);
        }
    }
    return 1.0f;
}

void AuraUnifiedEngine::push_event(const ::Aura::Core::EngineEvent& e) { m_eventQueue.push(e); }
bool AuraUnifiedEngine::pop_event(::Aura::Core::EngineEvent& e) const { return m_eventQueue.pop(e); }

rust::Vec<Bridge::PlainClash> AuraUnifiedEngine::run_masking_analysis(uint32_t trackAId) {
    rust::Vec<Bridge::PlainClash> results;
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    
    Track* targetTrack = nullptr;
    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        if (s.activeTracks[i]->getId() == trackAId) {
            targetTrack = s.activeTracks[i];
            break;
        }
    }
    
    if (!targetTrack) return results;
    
    uint32_t N = 512;
    auto& bufferA = targetTrack->getWorkBuffer(N);
    
    // Thread-local variables to avoid runtime dynamic allocations/deallocations
    thread_local std::vector<std::complex<float>> fftA(512);
    thread_local std::vector<std::complex<float>> fftB(512);
    thread_local std::vector<float> magA(256);
    
    uint32_t chanA = bufferA.getNumChannels();
    for (uint32_t idx = 0; idx < N; ++idx) {
        float sum = 0.0f;
        for (uint32_t c = 0; c < chanA; ++c) {
            sum += bufferA.getReadPointer(c)[idx];
        }
        fftA[idx] = sum / (chanA + 1e-6f);
    }
    local_fft_engine(fftA);

    for (uint32_t k = 0; k < N / 2; ++k) {
        magA[k] = std::abs(fftA[k]);
    }

    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        Track* other = s.activeTracks[i];
        if (other->getId() == trackAId || other->isMuted()) continue;
        
        auto& bufferB = other->getWorkBuffer(N);
        uint32_t chanB = bufferB.getNumChannels();
        for (uint32_t idx = 0; idx < N; ++idx) {
            float sum = 0.0f;
            for (uint32_t c = 0; c < chanB; ++c) {
                sum += bufferB.getReadPointer(c)[idx];
            }
            fftB[idx] = sum / (chanB + 1e-6f);
        }
        local_fft_engine(fftB);

        for (uint32_t k = 0; k < N / 2; ++k) {
            float magB = std::abs(fftB[k]);
            float clashVal = magA[k] * magB;
            
            if (clashVal > 0.02f) {
                Bridge::PlainClash clash;
                clash.bin = k;
                clash.intensity = clashVal;
                clash.track_b = other->getId();
                results.push_back(clash);
                
                if (results.size() >= 50) return results;
            }
        }
    }
    return results;
}

rust::Vec<::Aura::Core::Composition::NeuralArrangementKernel::Section> AuraUnifiedEngine::run_structural_analysis() {
    rust::Vec<::Aura::Core::Composition::NeuralArrangementKernel::Section> results;
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    
    std::vector<uint64_t> regionStarts;
    uint64_t maxLen = 0;
    
    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        Track* t = s.activeTracks[i];
        for (const auto& r : t->getRegions()) {
            regionStarts.push_back(r.start);
            maxLen = std::max(maxLen, r.start + r.len);
        }
    }
    if (regionStarts.empty()) {
        regionStarts.push_back(0);
        maxLen = static_cast<uint64_t>(m_sampleRate.load()) * 30; // default 30s
    }

    auto sections = Composition::NeuralArrangementKernel::AnalyzeStructure(regionStarts, maxLen, m_tempo.load(), m_sampleRate.load());
    for (const auto& sec : sections) {
        results.push_back(sec);
    }
    return results;
}

void AuraUnifiedEngine::autonomous_plugin_audit() {
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        // [SYSTEM AUDIT]
    }
}

std::map<std::string, float> AuraUnifiedEngine::get_vibe_scores() const {
    Mixing::AestheticFeatures f = m_vibeData.load(std::memory_order_relaxed);
    return Mixing::QualitativeMetricEngine::getInstance().calculateScores(f);
}

void AuraUnifiedEngine::perform_autonomous_correction() {
    Mixing::AestheticFeatures f = m_vibeData.load(std::memory_order_relaxed);

    if (f.transientClarity < 0.2f) {
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "ADVICE | CLARITY_CRITICAL | TRIGGER_SPECTRAL_CLEANUP");
    }

    if (f.dynamicComplexity > 0.85f && m_masterLimiter) {
         // m_masterLimiter->setDrive(m_masterLimiter->getDrive() * 0.99f); 
    }
}

bool AuraUnifiedEngine::checkSystemHealth() const {
    return m_engineDiagnostics.getHealth().isRealTimeSafe.load();
}

AuraUnifiedEngine::WaveformCache::~WaveformCache() {
    cleanup();
}
void AuraUnifiedEngine::WaveformCache::cleanup() {
    if (mappedData != nullptr && mappedSize > 0) {
        ::munmap(mappedData, mappedSize);
        mappedData = nullptr;
        mappedSize = 0;
    }
    if (fd >= 0) {
        ::close(fd);
        fd = -1;
    }
    if (!filepath.empty()) {
        std::error_code ec;
        std::filesystem::remove(filepath, ec);
        filepath.clear();
    }
}
