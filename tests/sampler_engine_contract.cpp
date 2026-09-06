#include "src/dsp/synthesis/sampler_engine.hpp"

#include <cassert>
#include <cmath>

int main() {
    Aura::Core::AudioBuffer sample(1, 64);
    for (uint32_t i = 0; i < sample.getNumSamples(); ++i)
        sample.getWritePointer(0)[i] = 0.25f;

    Aura::DSP::Synthesis::SamplerEngine engine;
    engine.prepareToPlay(48'000.0, 32);

    // Exercise the fixed free-voice stack through both allocation and
    // reclamation. A completed voice must be returned exactly once.
    for (uint32_t i = 0; i < 64; ++i)
        engine.noteOn(static_cast<uint8_t>(48 + (i % 24)), 100, &sample);

    Aura::Core::AudioBuffer output(2, 128);
    engine.process(output);
    for (uint32_t channel = 0; channel < output.getNumChannels(); ++channel)
        for (uint32_t frame = 0; frame < output.getNumSamples(); ++frame)
            assert(std::isfinite(output.getReadPointer(channel)[frame]));

    // The previous block consumed the short sample. Refill the free list and
    // verify that a new burst is audible instead of being rejected as stuck.
    engine.noteOn(60, 127, &sample);
    output.clear();
    engine.process(output);
    bool heard = false;
    for (uint32_t frame = 0; frame < output.getNumSamples(); ++frame)
        heard = heard || std::abs(output.getReadPointer(0)[frame]) > 1.0e-5f;
    assert(heard);
    return 0;
}
