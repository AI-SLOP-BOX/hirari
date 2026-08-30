#include <cassert>
#include <cmath>

#include "core/audio_processor_graph.hpp"

namespace {
class SilentLatencyProcessor final : public Aura::DSP::IProcessor {
public:
    explicit SilentLatencyProcessor(uint32_t latency) : m_latency(latency) {}

    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Aura::Core::AudioBuffer& buffer, Aura::Core::MidiBuffer&,
                 const Aura::DSP::ProcessContext&) noexcept override {
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

class InvalidOutputProcessor final : public Aura::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Aura::Core::AudioBuffer& buffer, Aura::Core::MidiBuffer&,
                 const Aura::DSP::ProcessContext&) noexcept override {
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
    using namespace Aura;
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
    for (uint32_t index = 0; index < 16; ++index) {
        assert(std::isfinite(buffer.getReadPointer(0)[index]));
        assert(std::isfinite(buffer.getReadPointer(1)[index]));
    }

    processor->setLatency(128);
    for (uint32_t index = 0; index < 16; ++index) {
        buffer.getWritePointer(0)[index] = 0.25f;
        buffer.getWritePointer(1)[index] = -0.5f;
    }
    graph.process(buffer, midi, context);
    for (uint32_t channel = 0; channel < 2; ++channel) {
        for (uint32_t index = 0; index < 16; ++index) {
            assert(std::isfinite(buffer.getReadPointer(channel)[index]));
        }
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
