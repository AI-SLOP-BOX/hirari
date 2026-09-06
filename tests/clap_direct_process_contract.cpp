#include "src/core/plugins/clap_host_processor.hpp"

#include <cassert>
#include <cmath>
#include <cstring>

int main(int argc, char** argv) {
    assert(argc == 2);

    Aura::Core::Plugins::CLAPHostProcessor processor;
    processor.prepareToPlay(48000.0, 16);
    assert(processor.loadClap(argv[1], 0));
    assert(processor.isOperational());
    assert(processor.hasProcessFunction());

    Aura::Core::AudioBuffer audio(2, 16);
    Aura::Core::MidiBuffer midi;
    const uint8_t ump[] = {
        0x40, 0x90, 0x3c, 0x7f, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00};
    midi.addEvent(3, ump, sizeof(ump));
    const uint8_t noteOn[] = {0x90, 64, 100};
    midi.addEvent(1, noteOn, sizeof(noteOn));
    for (uint32_t channel = 0; channel < audio.getNumChannels(); ++channel) {
        for (uint32_t frame = 0; frame < audio.getNumSamples(); ++frame)
            audio.getWritePointer(channel)[frame] = 1.0f;
    }

    Aura::DSP::ProcessContext context{};
    processor.process(audio, midi, context);
    assert(!processor.processFailed());
    assert(midi.size() == 4);
    assert(midi.getEvents()[0].sampleOffset == 1);
    assert(midi.getEvents()[1].sampleOffset == 1);
    assert(midi.getEvents()[2].size == sizeof(ump));
    assert(midi.getEvents()[2].sampleOffset == 3);
    assert(std::memcmp(midi.getEvents()[2].data, ump, sizeof(ump)) == 0);
    for (uint32_t channel = 0; channel < audio.getNumChannels(); ++channel)
        for (uint32_t frame = 0; frame < audio.getNumSamples(); ++frame)
            assert(std::abs(audio.getReadPointer(channel)[frame] - 0.5f) < 1.0e-6f);
    return 0;
}
