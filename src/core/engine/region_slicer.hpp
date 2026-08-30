#pragma once

#include <vector>
#include <memory>
#include <algorithm>
#include <cmath>
#include "../audio_region.hpp"
#include "../../dsp/analysis/transient_detector.hpp"

namespace Aura::Core::Engine {

/**
 * @brief RegionSlicer: Logic Pro-style 'Slice at Transients' capability.
 */
class RegionSlicer {
public:
    /**
     * @brief Slices a region based on rhythmic onsets and zero-crossings with industrial precision and rhythmic sovereignty.
     * INDUSTRIAL: Delegating transient analysis and cut-point optimization to the Rust 'SlicingOrchestrator'.
     */
    static std::vector<std::shared_ptr<AudioRegion>> sliceAtTransients(std::shared_ptr<AudioRegion> region, float sensitivity = 0.5f) {
        std::vector<std::shared_ptr<AudioRegion>> slices;
        if (!region || region->getSampleLength() == 0 || !region->getSource()) return slices;

        const auto source = region->getSource();
        const uint64_t sourceStart = region->getMeta().sampleOffset;
        if (source->getNumSamples() == 0 || source->getNumChannels() == 0 || sourceStart >= source->getNumSamples()) return slices;
        const uint64_t length = std::min(region->getSampleLength(), source->getNumSamples() - sourceStart);
        std::vector<float> mono(length);
        for (uint64_t i = 0; i < length; ++i) {
            const float left = source->getSample(0, sourceStart + i);
            const float right = source->getNumChannels() > 1 ? source->getSample(1, sourceStart + i) : left;
            mono[static_cast<size_t>(i)] = 0.5f * (left + right);
        }

        const float clampedSensitivity = std::clamp(std::isfinite(sensitivity) ? sensitivity : 0.5f, 0.0f, 1.0f);
        DSP::Analysis::TransientDetector detector(source->getSampleRate());
        const float threshold = 0.30f - clampedSensitivity * 0.28f;
        const auto transients = detector.analyze(mono.data(), mono.size(), threshold);

        std::vector<uint64_t> cuts;
        for (const auto& transient : transients) {
            if (transient.sampleIndex > 0 && transient.sampleIndex < length)
                cuts.push_back(transient.sampleIndex);
        }
        std::sort(cuts.begin(), cuts.end());
        cuts.erase(std::unique(cuts.begin(), cuts.end()), cuts.end());

        uint64_t previous = 0;
        for (const uint64_t cut : cuts) {
            if (cut > previous) {
                auto next = region->split(cut - previous);
                if (next) slices.push_back(std::move(next));
                previous = cut;
            }
        }
        slices.push_back(std::move(region));
        return slices;
    }
};

} // namespace Aura::Core::Engine
