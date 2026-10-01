#include <cassert>
#include <cmath>

#include "core/audio_processor_graph.hpp"

namespace {
class SilentLatencyProcessor final : public Hirari::DSP::IProcessor {
public:
    explicit SilentLatencyProcessor(uint32_t latency) : m_latency(latency) {}

    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Hirari::Core::AudioBuffer& buffer, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            float* samples = buffer.getWritePointer(channel);
            for (uint32_t index = 0; index < buffer.getNumSamples(); ++index) {
                samples[index] = 0.0f;
            }
        }
    }
    void reset() noexcept override {}
    uint32_t getLatencySamples() const noexcept override { return m_latency; }
    void setLatency(uint32_t latency) noexcept { m_latency = latency; }

private:
    uint32_t m_latency;
};

class InvalidOutputProcessor final : public Hirari::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Hirari::Core::AudioBuffer& buffer, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            float* samples = buffer.getWritePointer(channel);
            for (uint32_t index = 0; index < buffer.getNumSamples(); ++index) {
                samples[index] = index == 0 ? NAN : 0.25f;
            }
        }
    }
    void reset() noexcept override {}
};

}

int main() {
    using namespace Hirari;
    Core::AudioProcessorGraph graph;
    auto processor = std::make_shared<SilentLatencyProcessor>(4);
    processor->setMix(0.5f);
    graph.addNode(processor);
    graph.prepare(48000.0, 16);

    Core::AudioBuffer buffer(2, 16);
    for (uint32_t index = 0; index < 16; ++index) {
        buffer.getWritePointer(0)[index] = 1.0f;
        buffer.getWritePointer(1)[index] = 10.0f;
    }
    Core::MidiBuffer midi;
    DSP::ProcessContext context{};
    context.sampleRate = 48000.0;
    context.blockSize = 16;
    graph.process(buffer, midi, context);

    // The first block contains the delay-line transition. A silent source
    // channel must stay silent; sharing one delay line would leak the other
    // channel into it.
    //
    // `isfinite` alone cannot detect that leak, so assert the per-channel bounds
    // each channel's own input could produce. This processor emits silence, so
    // the output is the dry signal scaled by (1 - mix) = 0.5: channel 0 (input
    // 1.0) can only reach 0.5 and channel 1 (input 10.0) only 5.0. Any bleed of
    // the louder channel into the quieter one lands far outside these bounds.
    for (uint32_t index = 0; index < 16; ++index) {
        const float left = buffer.getReadPointer(0)[index];
        const float right = buffer.getReadPointer(1)[index];
        assert(std::isfinite(left));
        assert(std::isfinite(right));
        assert(left >= -1.0e-6f && left <= 0.5f + 1.0e-6f);
        assert(right >= -1.0e-6f && right <= 5.0f + 1.0e-6f);
    }

    processor->setLatency(128);
    for (uint32_t index = 0; index < 16; ++index) {
        buffer.getWritePointer(0)[index] = 0.25f;
        buffer.getWritePointer(1)[index] = -0.5f;
    }
    graph.process(buffer, midi, context);
    // The two inputs now have opposite signs, which makes any cross-channel
    // contamination impossible to miss: channel 0 can only ever be >= 0 and
    // channel 1 only ever <= 0.
    for (uint32_t index = 0; index < 16; ++index) {
        const float left = buffer.getReadPointer(0)[index];
        const float right = buffer.getReadPointer(1)[index];
        assert(std::isfinite(left));
        assert(std::isfinite(right));
        assert(left >= -1.0e-6f && left <= 0.125f + 1.0e-6f);
        assert(right >= -0.25f - 1.0e-6f && right <= 1.0e-6f);
    }

    Core::AudioProcessorGraph guarded;
    guarded.addNode(std::make_shared<InvalidOutputProcessor>());
    guarded.prepare(48000.0, 16);
    Core::AudioBuffer guardedBuffer(2, 16);
    DSP::ProcessContext guardedContext{};
    guardedContext.sampleRate = 48000.0;
    guarded.process(guardedBuffer, midi, guardedContext);
    assert(guarded.getSanitizedSampleCount() == 2);
    assert(guarded.getRejectedBlockCount() == 0);
    assert(std::isfinite(guardedBuffer.getReadPointer(0)[0]));
    assert(guarded.validateGraph());

    Core::AudioBuffer oversized(2, 32);
    guarded.process(oversized, midi, guardedContext);
    assert(guarded.getRejectedBlockCount() == 1);

    return 0;
}
