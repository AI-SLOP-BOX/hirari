#pragma once

#include "../../io/mmap_audio_file.hpp"
#include "../../core/rust_ffi.hpp"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>
#include <vector>

namespace Hirari::Core::DSP::Synthesis {

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

/** C++ instrument API adapter. Sampler state and audio rendering live in Rust. */
class HirariSamplerPro {
public:
    static constexpr int kMaxVoices = 64;

    explicit HirariSamplerPro(double sampleRate)
        : m_state(hirari_poly_sampler_create(sampleRate)) {}
    ~HirariSamplerPro() { hirari_poly_sampler_destroy(m_state); }
    HirariSamplerPro(const HirariSamplerPro&) = delete;
    HirariSamplerPro& operator=(const HirariSamplerPro&) = delete;

    void setZones(std::vector<SamplerZone> zones) {
        std::vector<HirariPolySamplerZoneView> views;
        views.reserve(zones.size());
        for (const auto& zone : zones) {
            views.push_back({
                zone.minNote, zone.maxNote, zone.minVelocity, zone.maxVelocity,
                zone.rootNote, zone.loopStart, zone.loopEnd, zone.loopEnabled,
                zone.sourceSampleRate,
                zone.left.data(), zone.left.size(),
                zone.right.data(), zone.right.size(),
            });
        }
        if (m_state) hirari_poly_sampler_set_zones(m_state, views.data(), views.size());
    }

    void noteOn(uint32_t note, float velocity) {
        if (m_state) hirari_poly_sampler_note_on(m_state, note, velocity);
    }
    void noteOff(uint32_t note) {
        if (m_state) hirari_poly_sampler_note_off(m_state, note);
    }

    bool setLoopRegion(uint64_t start, uint64_t end, bool enabled) noexcept {
        return hirari_poly_sampler_set_loop(m_state, start, end, enabled);
    }
    bool isLooping() const noexcept { return hirari_poly_sampler_is_looping(m_state); }
    uint64_t loopStart() const noexcept { return hirari_poly_sampler_loop_start(m_state); }
    uint64_t loopEnd() const noexcept { return hirari_poly_sampler_loop_end(m_state); }

    void process(float* outputLeft, float* outputRight, size_t frames);
    void processAdditive(float* outputLeft, float* outputRight, size_t frames);

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::DSP::Synthesis
