#include <cassert>
#include <cmath>
#include <limits>
#include <string>

#include "../src/platform/audio_device.hpp"

int main() {
    using Aura::Platform::AudioDevice;
    using Aura::Platform::SilentAudioDevice;

    SilentAudioDevice device;
    AudioDevice::Config first{48'000.0, 128, 2, 2};
    assert(device.initialize(first, nullptr, nullptr));
    assert(device.config().sampleRate == 48'000.0);
    assert(device.config().bufferSize == 128);
    assert(!device.start());
    assert(device.isSilentFallback());
    assert(!device.isRunning());

    // A failed device start must not prevent a later configuration generation
    // from being prepared for a different sample rate/block size.
    device.stop();
    AudioDevice::Config second{96'000.0, 512, 2, 2};
    assert(device.initialize(second, nullptr, nullptr));
    assert(device.config().sampleRate == 96'000.0);
    assert(device.config().bufferSize == 512);
    assert(!device.isRunning());

    // Invalid transitions fail closed and never claim a running device.
    AudioDevice::Config invalid{std::numeric_limits<double>::quiet_NaN(), 0, 2, 2};
    assert(!device.initialize(invalid, nullptr, nullptr));
    assert(!device.isRunning());
    assert(device.hasError());
    assert(std::string(device.lastError()).find("invalid") != std::string::npos);

    // A valid reinitialization must clear the previous failure state before
    // the backend is started again.
    AudioDevice::Config recovered{44'100.0, 64, 0, 2};
    assert(device.initialize(recovered, nullptr, nullptr));
    assert(!device.hasError());
    assert(device.config().sampleRate == recovered.sampleRate);
    assert(device.config().bufferSize == recovered.bufferSize);

    // Repeated device reconfiguration must remain deterministic and must not
    // leak the prior error state into a subsequent valid generation.
    for (int round = 0; round < 20; ++round) {
        const double rate = (round % 2 == 0) ? 44'100.0 : 96'000.0;
        const std::size_t frames = (round % 4 == 0) ? 128 : 512;
        AudioDevice::Config cycle{rate, static_cast<std::uint32_t>(frames), 0, 2};
        assert(device.initialize(cycle, nullptr, nullptr));
        assert(!device.hasError());
        assert(device.config().sampleRate == rate);
        assert(device.config().bufferSize == frames);
    }
    return 0;
}
