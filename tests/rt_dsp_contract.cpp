#include <cassert>
#include <cmath>
#include <limits>
#include <vector>

#include "../src/core/audio_buffer.hpp"
#include "../src/core/engine/automation_manager.hpp"
#include "../src/dsp/effects/auto_pitch_corrector.hpp"

int main() {
    using Hirari::Core::AudioBuffer;
    AudioBuffer::resetRealtimeResizeStats();
    AudioBuffer buffer(2, 16);
    AudioBuffer::setRTThread(true);
    assert(!buffer.resize(2, 128));
    AudioBuffer::setRTThread(false);
    assert(AudioBuffer::realtimeResizeAttempts() == 1);
    assert(AudioBuffer::lastRealtimeRequestedCapacity() >= 128);

    auto& automation = Hirari::Core::Engine::AutomationManager::getInstance();
    automation.prepareToPlay(48000.0);
    assert(automation.setTarget(3, 11, 1.0f));
    assert(std::abs(automation.getTarget(3, 11) - 1.0f) < 1.0e-6f);
    automation.process(64);
    assert(std::isfinite(automation.getValue(0, 0)));

    Hirari::Core::DSP::Effects::AutoPitchCorrector corrector(48000.0, 256);
    // detect_pitch() searches lags up to frames/2, so a 64-sample block can only
    // see frequencies above sampleRate/32 = 1500 Hz and can never resolve a
    // 440 Hz tone. Use a block long enough for the detector to find a ~109
    // sample period, otherwise the assertions below would be meaningless.
    constexpr uint32_t kPitchBlock = 2048;
    corrector.prepareToPlay(48000.0, kPitchBlock);
    Hirari::Core::MidiBuffer midi;
    Hirari::DSP::ProcessContext context{};
    context.sampleRate = 48000.0;
    context.blockSize = kPitchBlock;

    // An empty buffer must stay silent rather than injecting garbage.
    AudioBuffer silence(1, kPitchBlock);
    corrector.process(silence, midi, context);
    for (uint32_t i = 0; i < kPitchBlock; ++i) {
        assert(silence.getReadPointer(0)[i] == 0.0f);
    }

    // Feed a real 440 Hz tone. isfinite alone would pass even if process()
    // returned early and left the input untouched, so assert that the
    // detector actually identified the pitch and that the audio was processed.
    std::vector<float> input;
    input.reserve(kPitchBlock * 2);
    for (uint32_t block = 0; block < 2; ++block) {
        AudioBuffer stereo(2, kPitchBlock);
        for (uint32_t i = 0; i < kPitchBlock; ++i) {
            const float sample =
                std::sin(2.0 * 3.141592653589793 * 440.0 * (block * kPitchBlock + i) / 48000.0);
            stereo.getWritePointer(0)[i] = sample;
            stereo.getWritePointer(1)[i] = sample;
            input.push_back(sample);
        }
        corrector.process(stereo, midi, context);

        for (uint32_t channel = 0; channel < 2; ++channel) {
            for (uint32_t i = 0; i < kPitchBlock; ++i) {
                assert(std::isfinite(stereo.getReadPointer(channel)[i]));
            }
        }

        if (block == 1) {
            // The detector must resolve the fed frequency, not merely emit a
            // finite number.
            const float detected = corrector.getCurrentPitch();
            assert(detected > 430.0f && detected < 450.0f);
            // 440 Hz is already in tune, so the correction must be ~0. A large
            // value here would mean the scale-snap logic is misbehaving.
            assert(std::fabs(corrector.getCorrectionAmount()) < 1.0f);

            // The output must actually differ from the input: a pass-through
            // "fix" would satisfy every check above.
            bool differs = false;
            for (uint32_t i = 0; i < kPitchBlock; ++i) {
                if (stereo.getReadPointer(0)[i] != input[kPitchBlock + i]) {
                    differs = true;
                    break;
                }
            }
            assert(differs && "AutoPitchCorrector left the signal bit-identical");
        }
    }

    // Parameter round-trip must be reflected by the FFI surface.
    corrector.setParameter(0, 0.25f);
    assert(std::fabs(corrector.getParameter(0) - 0.25f) < 1.0e-4f);
    return 0;
}
