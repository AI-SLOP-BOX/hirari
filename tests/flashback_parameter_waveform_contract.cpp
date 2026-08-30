#include <cassert>
#include <cmath>
#include <vector>

#include "../src/core/engine/flashback_recorder.hpp"
#include "../src/core/parameter_tree.hpp"
#include "../src/ui/waveform_cache.hpp"

int main() {
    Aura::Core::Engine::FlashbackRecorder recorder(2);
    float left[8] = {0, 1, 2, 3, 4, 5, 6, 7};
    float right[8] = {7, 6, 5, 4, 3, 2, 1, 0};
    const float* inputs[2] = {left, right};
    recorder.write(inputs, 8);
    auto recalled = recorder.recall(1);
    assert(recalled && recalled->getNumChannels() == 2);
    assert(recalled->getNumSamples() == 44100);
    assert(std::abs(recalled->getReadPointer(0)[44092] - 0.0f) < 1.0e-6f);
    assert(std::abs(recalled->getReadPointer(0)[44099] - 7.0f) < 1.0e-6f);

    Aura::Core::ParameterTree tree;
    const uint32_t id = tree.registerParam("gain", 0.5f);
    assert(id != UINT32_MAX);
    assert(tree.registerParam("gain", 0.75f) == id);
    assert(tree.registerParam("bad", NAN) == UINT32_MAX);
    assert(std::abs(tree.getParam(id)->getTarget() - 0.75f) < 1.0e-6f);

    Aura::UI::WaveformLevel level;
    level.minPeaks = {-1.0f, -0.5f};
    level.maxPeaks = {1.0f, 0.5f};
    auto& cache = Aura::UI::WaveformCache::getInstance();
    cache.putWaveform(77, 128, level);
    Aura::UI::WaveformLevel copy;
    assert(cache.copyWaveform(77, 128, copy));
    assert(copy.maxPeaks == level.maxPeaks);
    cache.clearRegion(77);
    assert(!cache.copyWaveform(77, 128, copy));
    return 0;
}
