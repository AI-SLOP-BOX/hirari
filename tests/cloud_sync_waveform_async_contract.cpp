#include <cassert>
#include <vector>

#include "../src/graphics/ui_components/support/waveform_cache.hpp"

int main() {
    auto& cache = Hirari::UI::WaveformCache::getInstance();
    cache.clearRegion(9001);
    assert(cache.requestWaveform(9001, 2,
                                 std::vector<float>{-1.0f, 0.5f, 2.0f, 1.0f}));
    assert(cache.isPending(9001, 2));
    cache.waitForPending();
    assert(!cache.hasFailure(9001, 2));
    Hirari::UI::WaveformLevel level;
    assert(cache.copyWaveform(9001, 2, level));
    assert(level.minPeaks == std::vector<float>({-1.0f, 1.0f}));
    assert(level.maxPeaks == std::vector<float>({0.5f, 2.0f}));

    cache.clearRegion(9002);
    assert(cache.requestWaveform(9002, 1,
                                 std::vector<float>{-1.0f, 1.0f}));
    cache.clearRegion(9002);
    cache.waitForPending();
    assert(!cache.copyWaveform(9002, 1, level));
    return 0;
}
