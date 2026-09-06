#pragma once

#include "adsr_envelope.hpp"
#include "../../io/mmap_audio_file.hpp"
#include <atomic>
#include <vector>
#include <memory>
#include <cmath>
#include <algorithm>
#include <array>

namespace Aura::Core::DSP::Synthesis {

struct SamplerZone {
    uint32_t minNote = 0;
    uint32_t maxNote = 127;
    float minVelocity = 0.0f;
    float maxVelocity = 1.0f;
    uint32_t rootNote = 60;
    uint32_t loopStart = 0;
    uint32_t loopEnd = 0;
    bool loopEnabled = false;
    double sourceSampleRate = 0.0;
    
    std::vector<float> left;
    std::vector<float> right;
    std::shared_ptr<IO::MMapAudioFile> mmapFile;

    bool matches(uint32_t note, float velocity) const {
        return note >= minNote && note <= maxNote && velocity >= minVelocity && velocity <= maxVelocity;
    }
};

struct SamplerVoice {
    bool active = false;
    uint32_t note = 0;
    double playbackPos = 0.0;
    float currentSpeed = 1.0f;
    float targetSpeed = 1.0f; 
    double sampleRateRatio = 1.0;
    float slideRate = 0.0f;   
    float velocity = 1.0f;
    ADSREnvelope envelope;
    
    // HONEST FIX: Separate L/R states for SVF Filter (Stereo Separation)
    float filterLZ1 = 0.0f, filterLZ2 = 0.0f;
    float filterRZ1 = 0.0f, filterRZ2 = 0.0f;
    float filterCutoff = 1000.0f; 
    float filterResonance = 0.1f;

    float driftPhase = 0.0f;
    float driftSpeed = 0.01f;
    uint64_t startTime = 0; // For voice stealing

    const SamplerZone* currentZone = nullptr;
    // Keep the immutable zone table alive while this voice is rendering.
    // setZones() swaps tables atomically, so a raw pointer alone could dangle
    // at the exact moment a project reloads or edits its sample map.
    std::shared_ptr<const std::vector<SamplerZone>> zoneOwner;
    uint32_t loopStart = 0;
    uint32_t loopEnd = 0;
    bool loopEnabled = false;

    bool isAvailable() const { return !active; }
    void markAsAvailable() { 
        active = false; 
        envelope.reset(); 
        currentZone = nullptr; 
        zoneOwner.reset();
        filterLZ1 = filterLZ2 = filterRZ1 = filterRZ2 = 0.0f;
    }
    
    void updateSlide() {
        if (std::abs(currentSpeed - targetSpeed) < 1e-4f) {
            currentSpeed = targetSpeed;
            return;
        }
        currentSpeed += (targetSpeed - currentSpeed) * slideRate;
    }
};

class AuraSamplerPro {
public:
    static constexpr int kMaxVoices = 64; 

    explicit AuraSamplerPro(double sr)
        : m_sampleRate(std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        m_zones = std::make_shared<std::vector<SamplerZone>>();
        for (int i = 0; i < kMaxVoices; ++i) {
            m_voices[i].envelope.setSampleRate(m_sampleRate);
            m_voices[i].envelope.setParameters(0.005f, 0.1f, 0.8f, 0.3f);
        }
    }
    
    void setZones(std::vector<SamplerZone> zones) {
        zones.erase(std::remove_if(zones.begin(), zones.end(), [](const SamplerZone& zone) {
            return zone.minNote > zone.maxNote || zone.minNote > 127 || zone.maxNote > 127 ||
                   zone.minVelocity < 0.0f || zone.maxVelocity > 1.0f ||
                   zone.minVelocity > zone.maxVelocity ||
                   (!zone.right.empty() && zone.right.size() != zone.left.size()) ||
                   (zone.sourceSampleRate != 0.0 &&
                    (!std::isfinite(zone.sourceSampleRate) || zone.sourceSampleRate < 8'000.0 ||
                     zone.sourceSampleRate > 384'000.0)) ||
                   (zone.loopEnabled && (zone.loopStart >= zone.loopEnd ||
                                         zone.loopEnd > zone.left.size()));
        }), zones.end());
        // HONEST FIX: Ensure types match for atomic_store
        std::shared_ptr<const std::vector<SamplerZone>> newZones = std::make_shared<const std::vector<SamplerZone>>(std::move(zones));
        std::atomic_store(&m_zones, newZones);
        m_zoneRoundRobin.fill(0);
    }

    void noteOn(uint32_t note, float velocity) {
        // --- HONEST FIX: VOICE STEALING (LRU STYLE) ---
        // Address Point 5: Ensure new notes are heard even at max polyphony.
        int oldestIdx = -1;
        uint64_t oldestTime = UINT64_MAX;
        int freeIdx = -1;

        for (int i = 0; i < kMaxVoices; ++i) {
            if (m_voices[i].isAvailable()) {
                freeIdx = i;
                break;
            }
            if (m_voices[i].startTime < oldestTime) {
                oldestTime = m_voices[i].startTime;
                oldestIdx = i;
            }
        }

        int targetIdx = (freeIdx != -1) ? freeIdx : oldestIdx;
        if (targetIdx == -1) return; // Should not happen

        // Find matching zone
        auto zonesPtr = std::atomic_load(&m_zones);
        if (!zonesPtr) return;
        
        const SamplerZone* bestZone = nullptr;
        uint32_t bestVelocitySpan = UINT32_MAX;
        uint32_t bestKeySpan = UINT32_MAX;
        uint32_t candidates = 0;
        for (const auto& zone : *zonesPtr) {
            if (zone.matches(note, velocity)) {
                const uint32_t velocitySpan = static_cast<uint32_t>((zone.maxVelocity - zone.minVelocity) * 1'000'000.0f);
                const uint32_t keySpan = zone.maxNote - zone.minNote;
                if (velocitySpan < bestVelocitySpan ||
                    (velocitySpan == bestVelocitySpan && keySpan < bestKeySpan)) {
                    bestZone = &zone;
                    bestVelocitySpan = velocitySpan;
                    bestKeySpan = keySpan;
                    candidates = 1;
                } else if (velocitySpan == bestVelocitySpan && keySpan == bestKeySpan) {
                    ++candidates;
                }
            }
        }

        if (bestZone && candidates > 1) {
            const uint32_t target = m_zoneRoundRobin[note]++ % candidates;
            uint32_t ordinal = 0;
            for (const auto& zone : *zonesPtr) {
                if (!zone.matches(note, velocity)) continue;
                const uint32_t velocitySpan = static_cast<uint32_t>((zone.maxVelocity - zone.minVelocity) * 1'000'000.0f);
                if (velocitySpan != bestVelocitySpan || zone.maxNote - zone.minNote != bestKeySpan) continue;
                if (ordinal++ == target) {
                    bestZone = &zone;
                    break;
                }
            }
        }
        
        if (bestZone) {
            triggerVoice(targetIdx, note, velocity, bestZone, zonesPtr);
            m_voices[targetIdx].startTime = ++m_globalTime;
        }
    }

    void noteOff(uint32_t note) {
        for (int i = 0; i < kMaxVoices; ++i) {
            if (m_voices[i].active && m_voices[i].note == note) {
                if (m_voices[i].envelope.getState() != ADSR_RELEASE) {
                    m_voices[i].envelope.triggerOff();
                }
            }
        }
    }

    // Configure a fallback loop for zones that do not carry their own loop
    // metadata. A disabled loop clears the previous range atomically.
    bool setLoopRegion(uint64_t start, uint64_t end, bool enabled) noexcept {
        if (!enabled) {
            m_loopStart = 0;
            m_loopEnd = 0;
            m_isLooping = false;
            return true;
        }
        if (start >= end || end - start < 2) return false;
        m_loopStart = start;
        m_loopEnd = end;
        m_isLooping = true;
        return true;
    }

    bool isLooping() const noexcept { return m_isLooping; }
    uint64_t loopStart() const noexcept { return m_loopStart; }
    uint64_t loopEnd() const noexcept { return m_loopEnd; }

    void process(float* outputL, float* outputR, size_t numFrames);
    // Layering variant: preserves audio already rendered by another engine.
    void processAdditive(float* outputL, float* outputR, size_t numFrames);

private:
    std::atomic<uint64_t> m_loopStart{0}, m_loopEnd{0};
    std::atomic<bool> m_isLooping{false};
    double m_sampleRate;
    uint64_t m_globalTime = 0; // For voice stealing
    
    // HONEST FIX: Zone list is now a shared pointer for RT safety.
    std::shared_ptr<const std::vector<SamplerZone>> m_zones;
    SamplerVoice m_voices[kMaxVoices];
    std::array<uint32_t, 128> m_zoneRoundRobin{};

    static float interpolateSample(const float* data, size_t length, double position,
                                   bool looping, uint64_t loopStart, uint64_t loopEnd) noexcept {
        if (!data || length == 0 || !std::isfinite(position)) return 0.0f;
        const auto at = [&](int64_t rawIndex) noexcept {
            int64_t index = rawIndex;
            if (looping && loopEnd > loopStart + 1) {
                const int64_t start = static_cast<int64_t>(loopStart);
                const int64_t end = static_cast<int64_t>(loopEnd);
                const int64_t span = end - start;
                if (index < start) {
                    index = end - ((start - index) % span);
                    if (index == end) index = start;
                } else if (index >= end) {
                    index = start + ((index - start) % span);
                }
            } else {
                index = std::clamp<int64_t>(index, 0, static_cast<int64_t>(length - 1));
            }
            const float value = data[static_cast<size_t>(index)];
            return std::isfinite(value) ? value : 0.0f;
        };
        const int64_t i = static_cast<int64_t>(std::floor(position));
        const float t = static_cast<float>(position - static_cast<double>(i));
        const float y0 = at(i - 1);
        const float y1 = at(i);
        const float y2 = at(i + 1);
        const float y3 = at(i + 2);
        const float c0 = y1;
        const float c1 = 0.5f * (y2 - y0);
        const float c2 = y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3;
        const float c3 = 0.5f * (y3 - y0) + 1.5f * (y1 - y2);
        const float result = ((c3 * t + c2) * t + c1) * t + c0;
        return std::isfinite(result) ? result : y1;
    }

    void triggerVoice(int idx, uint32_t note, float velocity, const SamplerZone* zone,
                      const std::shared_ptr<const std::vector<SamplerZone>>& owner) {
        m_voices[idx].active = true;
        m_voices[idx].note = note;
        m_voices[idx].playbackPos = 0.0;
        m_voices[idx].velocity = velocity;
        m_voices[idx].currentZone = zone;
        m_voices[idx].zoneOwner = owner;
        m_voices[idx].loopStart = zone ? zone->loopStart : 0;
        m_voices[idx].loopEnd = zone ? zone->loopEnd : 0;
        m_voices[idx].loopEnabled = zone && zone->loopEnabled &&
                                    zone->loopStart < zone->loopEnd;
        
        float root = zone ? static_cast<float>(zone->rootNote) : 60.0f;
        float speed = std::pow(2.0f, (static_cast<float>(note) - root) / 12.0f);
        m_voices[idx].currentSpeed = speed;
        m_voices[idx].targetSpeed = speed;
        m_voices[idx].sampleRateRatio = (zone && std::isfinite(zone->sourceSampleRate) &&
                                         zone->sourceSampleRate >= 8'000.0 &&
                                         zone->sourceSampleRate <= 384'000.0 && m_sampleRate > 0.0)
                                            ? zone->sourceSampleRate / m_sampleRate : 1.0;
        m_voices[idx].slideRate = 0.005f; 
        
        m_voices[idx].envelope.triggerOn();
    }
};

} // namespace Aura::Core::DSP::Synthesis
