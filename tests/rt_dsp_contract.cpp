#include <cassert>
#include <cmath>
#include <limits>

#include "../src/core/audio_buffer.hpp"
#include "../src/core/engine/automation_manager.hpp"
#include "../src/dsp/effects/auto_pitch_corrector.hpp"

int main() {
    using Aura::Core::AudioBuffer;
    AudioBuffer::resetRealtimeResizeStats();
    AudioBuffer buffer(2, 16);
    AudioBuffer::setRTThread(true);
    assert(!buffer.resize(2, 128));
    AudioBuffer::setRTThread(false);
    assert(AudioBuffer::realtimeResizeAttempts() == 1);
    assert(AudioBuffer::lastRealtimeRequestedCapacity() >= 128);

    auto& automation = Aura::Core::Engine::AutomationManager::getInstance();
    automation.prepareToPlay(48000.0);
    assert(automation.setTarget(3, 11, 1.0f));
    assert(std::abs(automation.getTarget(3, 11) - 1.0f) < 1.0e-6f);
    automation.process(64);
    assert(std::isfinite(automation.getValue(0, 0)));

    Aura::Core::DSP::Effects::AutoPitchCorrector corrector(48000.0, 256);
    corrector.prepareToPlay(48000.0, 64);
    Aura::Core::MidiBuffer midi;
    Aura::DSP::ProcessContext context{};
    context.sampleRate = 48000.0;
    context.blockSize = 64;
    AudioBuffer mono(1, 64);
    corrector.process(mono, midi, context);
    AudioBuffer stereo(2, 64);
    for (uint32_t i = 0; i < 64; ++i) {
        stereo.getWritePointer(0)[i] = std::sin(2.0 * 3.141592653589793 * 440.0 * i / 48000.0);
        stereo.getWritePointer(1)[i] = stereo.getReadPointer(0)[i];
    }
    corrector.process(stereo, midi, context);
    for (uint32_t i = 0; i < 64; ++i) {
        assert(std::isfinite(stereo.getReadPointer(0)[i]));
        assert(std::isfinite(stereo.getReadPointer(1)[i]));
    }
    return 0;
}
