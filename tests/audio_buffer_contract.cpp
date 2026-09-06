#include "../src/core/audio_buffer.hpp"

#include <cassert>
#include <cmath>

int main() {
    Aura::Core::AudioBuffer stereo(2, 4);
    stereo.getWritePointer(0)[0] = 0.25f;
    stereo.getWritePointer(1)[0] = -0.25f;

    // Invalid raw bridge input must be rejected without touching audio.
    stereo.addFrom(nullptr, nullptr, 4);
    assert(stereo.getReadPointer(0)[0] == 0.25f);
    assert(stereo.getReadPointer(1)[0] == -0.25f);

    const float left[4] = {0.5f, 0.0f, 0.0f, 0.0f};
    stereo.addFrom(left, nullptr, 4);
    assert(stereo.getReadPointer(0)[0] == 0.25f);
    assert(stereo.getReadPointer(1)[0] == -0.25f);

    const float right[4] = {-0.5f, 0.0f, 0.0f, 0.0f};
    stereo.addFrom(left, right, 4);
    assert(std::abs(stereo.getReadPointer(0)[0] - 0.75f) < 1.0e-6f);
    assert(std::abs(stereo.getReadPointer(1)[0] + 0.75f) < 1.0e-6f);

    stereo.applyGain(std::numeric_limits<float>::quiet_NaN());
    stereo.applyGain(std::numeric_limits<float>::infinity());
    assert(std::isfinite(stereo.getReadPointer(0)[0]));
    assert(std::isfinite(stereo.getReadPointer(1)[0]));
    return 0;
}
