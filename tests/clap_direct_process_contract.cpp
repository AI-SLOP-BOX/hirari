#include "src/core/plugins/clap_host_processor.hpp"

#include <cassert>
#include <cmath>

int main(int argc, char** argv) {
    assert(argc == 2);

    Aura::Core::Plugins::CLAPHostProcessor processor;
    processor.prepareToPlay(48000.0, 16);
    assert(processor.loadClap(argv[1], 0));
    assert(processor.isOperational());
    assert(processor.hasProcessFunction());

    Aura::Core::AudioBuffer audio(2, 16);
    Aura::Core::MidiBuffer midi;
    for (uint32_t channel = 0; channel < audio.getNumChannels(); ++channel) {
        for (uint32_t frame = 0; frame < audio.getNumSamples(); ++frame)
            audio.getWritePointer(channel)[frame] = 1.0f;
    }

    Aura::DSP::ProcessContext context{};
    processor.process(audio, midi, context);
    assert(!processor.processFailed());
    for (uint32_t channel = 0; channel < audio.getNumChannels(); ++channel)
        for (uint32_t frame = 0; frame < audio.getNumSamples(); ++frame)
            assert(std::abs(audio.getReadPointer(channel)[frame] - 0.5f) < 1.0e-6f);
    return 0;
}
