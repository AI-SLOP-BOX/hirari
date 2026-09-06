#include "../src/core/engine/track.hpp"

#include <cassert>
#include <cmath>
#include <limits>
#include <memory>

int main() {
    using Aura::Core::AudioBuffer;
    using Aura::Core::Engine::Region;
    using Aura::Core::Engine::Track;

    auto audio = std::make_shared<AudioBuffer>(2, 8);
    for (uint32_t i = 0; i < 8; ++i) {
        audio->getWritePointer(0)[i] = 0.25f;
        audio->getWritePointer(1)[i] = 0.25f;
    }
    Region region{};
    region.id = 1;
    region.start = 0;
    region.len = 8;
    region.audio = audio;
    region.rangeEdits.push_back({0, 100, 0.5f, 999, 999});
    region.rangeEdits.push_back({8, 12, 1.0f, 0, 0});
    region.rangeEdits.push_back({1, 2, std::numeric_limits<float>::quiet_NaN(), 0, 0});

    Track track(1, "range-contract", Track::Audio);
    track.addRegion(region);
    const auto regions = track.getRegions();
    assert(regions.size() == 1);
    assert(regions.front().rangeEdits.size() == 1);
    assert(regions.front().rangeEdits.front().end == 8);
    assert(regions.front().rangeEdits.front().fadeIn == 8);
    assert(regions.front().rangeEdits.front().fadeOut == 8);

    std::vector<Region::RangeEdit> replacement{{0, 4, 1.0f, 99, 99}};
    assert(track.replaceRegionRangeEdits(1, replacement));
    const auto replaced = track.regionRangeEdits(1);
    assert(replaced.size() == 1 && replaced.front().fadeIn == 4 && replaced.front().fadeOut == 4);
    std::vector<Region::RangeEdit> invalid{{0, 9, 1.0f, 0, 0}};
    assert(!track.replaceRegionRangeEdits(1, invalid));
    return 0;
}
