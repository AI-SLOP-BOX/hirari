#include <cassert>
#include <cmath>
#include <vector>

#include "../src/core/plugins/au_host_processor.hpp"

static void run_format(uint32_t channels) {
    Aura::Core::Plugins::AUHostProcessor processor;
    assert(processor.loadPlugin(kAudioUnitType_Effect, 'dcmp', 'appl'));
    processor.setNumChannels(channels);
    processor.prepareToPlay(48000.0, 128);
    assert(processor.isOperational());

    Aura::Core::AudioBuffer buffer(channels, 128);
    for (uint32_t channel = 0; channel < channels; ++channel) {
        for (uint32_t frame = 0; frame < 128; ++frame)
            buffer.getWritePointer(channel)[frame] = frame == 0 ? 0.25f : 0.0f;
    }
    Aura::Core::MidiBuffer midi;
    Aura::DSP::ProcessContext context{};
    context.sampleRate = 48000.0;
    context.blockSize = 128;
    // Parameter automation must use AU's realtime scheduling API, never
    // AudioUnitSetParameter from the render callback.
    processor.scheduleParameter(0, -18.0f, 0);
    processor.process(buffer, midi, context);
    assert(!processor.takeParameterScheduleFailure());
    const auto state = processor.getState();
    assert(!state.empty());
    processor.setState(state);
    assert(!processor.takeStateRestoreFailure());
    processor.setState(std::vector<uint8_t>{0x00, 0x01, 0x02, 0x03});
    assert(processor.takeStateRestoreFailure());
    // Oversized state is rejected before touching the AU and is observable by
    // the control side instead of being silently truncated.
    assert(!processor.setFullState(std::vector<uint8_t>(4u * 1024u * 1024u + 1u)));
    assert(processor.takeStateRestoreFailure());
    processor.reset();
    for (uint32_t channel = 0; channel < channels; ++channel)
        for (uint32_t frame = 0; frame < 128; ++frame)
            assert(std::isfinite(buffer.getReadPointer(channel)[frame]));
}

int main() {
    run_format(1);
    run_format(2);
    return 0;
}
