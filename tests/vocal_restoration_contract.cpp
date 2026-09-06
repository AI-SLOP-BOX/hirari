#include "../src/scae/vocal_restoration.hpp"

#include <cmath>
#include <cstdio>
#include <stdexcept>

namespace {
bool checkFiniteAndIdentity(Aura::Core::AudioBuffer& buffer,
                            const float* expected,
                            uint32_t samples,
                            float tolerance) {
    const float* actual = buffer.getReadPointer(0);
    for (uint32_t i = 0; i < samples; ++i) {
        if (!std::isfinite(actual[i]) || std::abs(actual[i] - expected[i]) > tolerance) {
            std::fprintf(stderr, "restoration mismatch at %u: %.9f (expected %.9f)\n",
                         i, actual[i], expected[i]);
            return false;
        }
    }
    return true;
}
} // namespace

int main() {
    using Aura::Core::AudioBuffer;
    using Aura::SCAE::Intelligence::VocalRestorationMaster;

    bool rejectedInvalidFft = false;
    try {
        Aura::DSP::Analysis::FFTEngine invalid(0);
        (void)invalid;
    } catch (const std::invalid_argument&) {
        rejectedInvalidFft = true;
    }
    if (!rejectedInvalidFft) return 4;

    // The old fixed 1/1.5 normalization failed for clips shorter than one
    // FFT frame and attenuated both finite-render edges.  Zero-intensity
    // restoration must be an identity transform for arbitrary clip lengths.
    constexpr uint32_t shortSamples = 17;
    AudioBuffer shortClip(1, shortSamples);
    float expectedShort[shortSamples]{};
    for (uint32_t i = 0; i < shortSamples; ++i) {
        expectedShort[i] = 0.1f + 0.03f * static_cast<float>(i);
        shortClip.getWritePointer(0)[i] = expectedShort[i];
    }

    VocalRestorationMaster restoration(64);
    restoration.restore(shortClip, 0.0f);
    if (!checkFiniteAndIdentity(shortClip, expectedShort, shortSamples, 2.0e-5f)) return 1;

    // Exercise a non-frame-aligned stereo block and then a channel-count
    // change on the same processor instance; both used to risk stale overlap
    // state or an out-of-bounds channel access.
    constexpr uint32_t blockSamples = 257;
    AudioBuffer stereo(2, blockSamples);
    float expectedStereo[2][blockSamples]{};
    for (uint32_t ch = 0; ch < stereo.getNumChannels(); ++ch) {
        float* data = stereo.getWritePointer(ch);
        for (uint32_t i = 0; i < blockSamples; ++i) {
            expectedStereo[ch][i] = std::sin(0.017f * static_cast<float>(i + ch));
            data[i] = expectedStereo[ch][i];
        }
    }
    restoration.restore(stereo, 0.0f);
    for (uint32_t ch = 0; ch < stereo.getNumChannels(); ++ch) {
        const float* data = stereo.getReadPointer(ch);
        for (uint32_t i = 0; i < blockSamples; ++i) {
            if (!std::isfinite(data[i]) || std::abs(data[i] - expectedStereo[ch][i]) > 1.0e-3f) {
                std::fprintf(stderr, "stereo mismatch ch=%u sample=%u: %.9f (expected %.9f)\n",
                             ch, i, data[i], expectedStereo[ch][i]);
                return 2;
            }
        }
    }

    AudioBuffer mono(1, 31);
    mono.getWritePointer(0)[0] = 0.5f;
    restoration.restore(mono, 0.4f);
    if (!std::isfinite(mono.getReadPointer(0)[0])) return 3;

    std::puts("vocal restoration contract passed");
    return 0;
}
