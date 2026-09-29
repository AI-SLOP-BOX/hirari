#pragma once

#include <chrono>
#include <algorithm>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>
#include "../core/audio_region.hpp"
#include "../core/rust_ffi.hpp"

namespace Hirari::Rendering {

class WaveformOverview {
public:
    struct Peak { float min = 0.0f; float max = 0.0f; };
    struct LOD { uint32_t ratio = 0; std::vector<float> minData; std::vector<float> maxData; };

    explicit WaveformOverview(std::shared_ptr<::Hirari::Core::IAudioSource> source)
        : m_source(std::move(source)),
          m_state(m_source ? hirari_waveform_overview_create(
              m_source.get(), &sourceSampleCount, &sourceSample) : nullptr) {}
    ~WaveformOverview() { hirari_waveform_overview_destroy(m_state); }

    WaveformOverview(const WaveformOverview&) = delete;
    WaveformOverview& operator=(const WaveformOverview&) = delete;

    [[deprecated("Use copyLOD or copyBestLOD")]]
    const float* getMinData() const noexcept { return nullptr; }
    [[deprecated("Use copyLOD or copyBestLOD")]]
    const float* getMaxData() const noexcept { return nullptr; }

    size_t getNumPeaks() const {
        uint32_t ratio = 0;
        size_t count = 0;
        return hirari_waveform_overview_lod_info(m_state, 0, &ratio, &count) ? count : 0;
    }

    bool copyLOD(uint32_t ratio, LOD& destination) const {
        for (uint32_t i = 0; i < hirari_waveform_overview_lod_count(m_state); ++i) {
            uint32_t candidateRatio = 0;
            size_t count = 0;
            if (hirari_waveform_overview_lod_info(m_state, i, &candidateRatio, &count) &&
                candidateRatio == ratio) return copyLODAt(i, destination);
        }
        return false;
    }

    bool copyLODs(std::vector<LOD>& destination) const {
        destination.clear();
        const uint32_t count = hirari_waveform_overview_lod_count(m_state);
        destination.reserve(count);
        for (uint32_t i = 0; i < count; ++i) {
            LOD lod;
            if (!copyLODAt(i, lod)) { destination.clear(); return false; }
            destination.push_back(std::move(lod));
        }
        return !destination.empty();
    }

    bool copyBestLOD(uint32_t pixels, LOD& destination) const {
        std::vector<LOD> lods;
        if (!copyLODs(lods)) return false;
        const LOD* best = &lods.front();
        uint64_t bestDistance = UINT64_MAX;
        for (const auto& lod : lods) {
            const uint64_t points = lod.minData.size();
            const uint64_t distance = points > pixels ? points - pixels : pixels - points;
            if (distance < bestDistance) { bestDistance = distance; best = &lod; }
        }
        destination = *best;
        return true;
    }

    bool failed() const noexcept { return hirari_waveform_overview_failed(m_state); }
    bool waitUntilReady(std::chrono::milliseconds timeout) const {
        return hirari_waveform_overview_wait(
            m_state, static_cast<uint64_t>(std::max<int64_t>(0, timeout.count())));
    }
    std::string lastError() const {
        const size_t size = hirari_waveform_overview_copy_error(m_state, nullptr, 0);
        if (size == 0) return {};
        std::vector<char> buffer(size + 1, '\0');
        hirari_waveform_overview_copy_error(m_state,
            reinterpret_cast<uint8_t*>(buffer.data()), buffer.size());
        return buffer.data();
    }

private:
    static bool sourceSampleCount(void* opaque, uint64_t* destination) noexcept {
        if (!destination) return false;
        auto* source = static_cast<::Hirari::Core::IAudioSource*>(opaque);
        if (!source) return false;
        try { *destination = source->getNumSamples(); return true; }
        catch (...) { return false; }
    }

    static bool sourceSample(void* opaque, uint64_t index, float* destination) noexcept {
        if (!destination) return false;
        auto* source = static_cast<::Hirari::Core::IAudioSource*>(opaque);
        if (!source) return false;
        try { *destination = source->getSample(0, index); return true; }
        catch (...) { return false; }
    }

    bool copyLODAt(uint32_t index, LOD& destination) const {
        uint32_t ratio = 0;
        size_t count = 0;
        if (!hirari_waveform_overview_lod_info(m_state, index, &ratio, &count)) return false;
        LOD result;
        result.ratio = ratio;
        result.minData.resize(count);
        result.maxData.resize(count);
        if (!hirari_waveform_overview_copy_lod(m_state, index,
                result.minData.data(), result.maxData.data(), count)) return false;
        destination = std::move(result);
        return true;
    }

    std::shared_ptr<::Hirari::Core::IAudioSource> m_source;
    void* m_state = nullptr;
};

} // namespace Hirari::Rendering
