#include "../src/core/audio_buffer.hpp"

#include <cassert>
#include <cmath>

int main() {
    Hirari::Core::AudioBuffer stereo(2, 4);
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

    // Rust owns the dimensions and the channel view.
    assert(stereo.getNumChannels() == 2);
    assert(stereo.getNumSamples() == 4);
    assert(stereo.getReadPointer(2) == nullptr);
    assert(stereo.getWritePointer(2) == nullptr);
    assert(stereo.getReadPointer(0, 99) == stereo.getReadPointer(0) + 4);
    assert(stereo.getArrayOfWritePointers()[1] == stereo.getWritePointer(1));
    assert(stereo.getArrayOfReadPointers()[0] == stereo.getReadPointer(0));

    stereo.getWritePointer(0)[1] = std::numeric_limits<float>::infinity();
    assert(stereo.sanitizeNonFinite() == 1);
    assert(stereo.getReadPointer(0)[1] == 0.0f);
    stereo.clear(1);
    assert(stereo.getReadPointer(0)[0] == 0.0f);

    Hirari::Core::AudioBuffer other(2, 4);
    other.getWritePointer(0)[0] = 2.0f;
    other.getWritePointer(1)[0] = -2.0f;
    stereo.addFrom(other, 4);
    assert(stereo.getReadPointer(0)[0] == 2.0f);
    assert(stereo.getReadPointer(1)[0] == -2.0f);

    float externalLeft[4] = {1.0f, 0.0f, 0.0f, 0.0f};
    float externalRight[4] = {-1.0f, 0.0f, 0.0f, 0.0f};
    float* planes[2] = {externalLeft, externalRight};
    Hirari::Core::AudioBuffer wrapped;
    wrapped.wrapChannels(planes, 2, 4);
    assert(wrapped.getNumChannels() == 2);
    assert(wrapped.getWritePointer(0) == externalLeft);
    wrapped.wrapChannels(nullptr, 0, 0);
    assert(wrapped.getNumChannels() == 0);

    Hirari::Core::AudioBuffer moved(std::move(other));
    assert(moved.getNumChannels() == 2);
    assert(moved.getReadPointer(0)[0] == 2.0f);
    assert(other.getNumChannels() == 0);
    return 0;
}
