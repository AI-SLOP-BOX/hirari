#include "../src/core/engine/track.hpp"

#include <array>
#include <cassert>
#include <cmath>
#include <limits>
#include <memory>

int main() {
    using Hirari::Core::AudioBuffer;
    using Hirari::Core::Engine::Region;
    using Hirari::Core::Engine::Track;

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
    const std::array<HirariRegionRangeEdit, 3> seed{{
        {0, 100, 0.5f, 999, 999},
        {8, 12, 1.0f, 0, 0},
        {1, 2, std::numeric_limits<float>::quiet_NaN(), 0, 0}}};
    assert(hirari_region_clip_edits_replace_range_edits(
        region.clipEdits.get(), seed.data(), seed.size(), region.len, false));

    Track track(1, "range-contract", Track::Audio);
    track.addRegion(region);
    const auto regions = track.getRegions();
    assert(regions.size() == 1);
    assert(regions.front().clip()->rangeEditCount == 1);
    assert(regions.front().clip()->rangeEdits->end == 8);
    assert(regions.front().clip()->rangeEdits->fade_in == 8);
    assert(regions.front().clip()->rangeEdits->fade_out == 8);

    const std::array<Region::RangeEdit, 1> replacement{{{0, 4, 1.0f, 99, 99}}};
    assert(track.replaceRegionRangeEdits(1, replacement.data(), replacement.size()));
    const auto replaced = track.regionRangeEdits(1);
    assert(replaced.size() == 1 && replaced.front().fade_in == 4 &&
           replaced.front().fade_out == 4);
    const std::array<Region::RangeEdit, 1> invalid{{{0, 9, 1.0f, 0, 0}}};
    assert(!track.replaceRegionRangeEdits(1, invalid.data(), invalid.size()));
    return 0;
}
