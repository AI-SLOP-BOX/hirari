#include <cassert>
#include <vector>

#include "../src/ui/waveform_cache.hpp"

int main() {
    auto& cache = Aura::UI::WaveformCache::getInstance();
    Aura::UI::WaveformLevel level;

    cache.clearRegion(9101);
    assert(cache.requestWaveform(9101, 2,
                                 std::vector<float>{-1.0f, 0.25f, 0.5f, 1.0f}));
    // Invalidate while the worker may still be running. Its completion must
    // not publish data into the next generation or clear its pending state.
    cache.clearRegion(9101);
    assert(!cache.copyWaveform(9101, 2, level));
    cache.waitForPending();
    assert(!cache.copyWaveform(9101, 2, level));
    // A stale completion must release its own pending slot; otherwise this
    // new generation would be rejected forever as "already pending".
    assert(cache.requestWaveform(9101, 2,
                                 std::vector<float>{-0.25f, 0.0f, 0.25f, 0.5f}));
    cache.waitForPending();
    cache.clearRegion(9101);

    assert(cache.requestWaveform(9101, 2,
                                 std::vector<float>{-0.5f, 0.25f, 0.75f, 0.5f}));
    cache.waitForPending();
    assert(cache.copyWaveform(9101, 2, level));
    assert(level.minPeaks == std::vector<float>({-0.5f, 0.5f}));
    assert(level.maxPeaks == std::vector<float>({0.25f, 0.75f}));
    return 0;
}
