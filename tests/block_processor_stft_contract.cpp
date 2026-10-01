// BlockProcessor is the fixed-size STFT framing bridge used by reverb, pitch
// shifting and spectral repair. This contract checks the property every such
// caller depends on: a constant input must come back out unchanged, for every
// supported overlap ratio. The previous implementation accumulated the
// overlap-add result and read it back through an independently shifted cursor,
// so a frame's tail was consumed before it was fully accumulated (-8.5 dB at
// 50% overlap, +3.2 dB at 75%) and the error was hop-dependent rather than a
// constant gain.
#include "src/dsp/block_processor.hpp"

#include <cassert>
#include <cmath>
#include <cstdio>
#include <functional>
#include <vector>

namespace {

// BlockProcessor applies the analysis and synthesis windows itself, so an
// identity callback isolates the framing/overlap-add behaviour.
void identityCallback(const std::vector<float>& in, std::vector<float>& out) { out = in; }

void checkPassThrough(uint32_t processSize, uint32_t hopSize, const char* label) {
    Hirari::DSP::BlockProcessor processor(processSize, hopSize);

    constexpr uint32_t kTotal = 16384;
    std::vector<float> buffer(kTotal, 1.0f);  // DC input

    processor.process(buffer.data(), kTotal, identityCallback);

    const uint32_t latency = processor.getLatencySamples();
    // Measure well past the priming latency and the first transient frames.
    const uint32_t start = latency + processSize * 4;
    assert(start < kTotal && "latency leaves no measurable region");

    double sum = 0.0;
    uint32_t count = 0;
    for (uint32_t i = start; i < kTotal; ++i) {
        sum += buffer[i];
        ++count;
    }
    const double mean = sum / static_cast<double>(count);
    std::printf("%s: N=%u H=%u mean=%.6f latency=%u\n", label, processSize, hopSize, mean, latency);

    assert(std::fabs(mean - 1.0) < 0.02 && "BlockProcessor is not COLA-correct");
    // Output must also be finite everywhere; NaN/Inf from the accumulator would
    // silently poison downstream spectral state.
    for (float sample : buffer) {
        assert(std::isfinite(sample));
    }
}

}  // namespace

int main() {
    // Latency must be the real algorithmic delay (one frame minus one hop),
    // not the frame size: over-reporting misaligns plugin delay compensation.
    {
        Hirari::DSP::BlockProcessor processor(2048, 1024);
        assert(processor.getLatencySamples() == 2048 - 1024);
    }
    checkPassThrough(2048, 1024, "50% overlap");
    checkPassThrough(2048, 512, "75% overlap");
    checkPassThrough(2048, 256, "87.5% overlap");
    return 0;
}