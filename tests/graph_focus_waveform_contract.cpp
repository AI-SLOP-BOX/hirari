#include <cassert>
#include <cmath>
#include <memory>
#include <chrono>
#include <thread>
#include <limits>
#include <vector>

#include "../src/core/audio_processor_graph.hpp"
#include "../src/rendering/waveform_overview.hpp"
#include "../src/graphics/ui_components/support/main/focus_manager.hpp"

class Source final : public Hirari::Core::IAudioSource {
public:
    float getSample(uint32_t channel, uint64_t index) const override {
        return channel == 0 && index < samples.size() ? samples[index] : 0.0f;
    }
    uint64_t getNumSamples() const override { return samples.size(); }
    uint32_t getNumChannels() const override { return 1; }
    std::vector<float> samples{0.25f, 0.5f, 0.75f, 1.0f};
};

class Passthrough final : public Hirari::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Hirari::Core::AudioBuffer&, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {}
    void reset() noexcept override {}
};

class NaNProcessor final : public Hirari::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Hirari::Core::AudioBuffer& buffer, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* samples = buffer.getWritePointer(c);
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i)
                samples[i] = std::numeric_limits<float>::quiet_NaN();
        }
    }
    void reset() noexcept override {}
};

class KeyRecorder final : public Hirari::UI::Main::FocusManager::IEventHandler {
public:
    bool handleKey(int key, bool pressed) override {
        lastKey = key;
        lastPressed = pressed;
        ++calls;
        return true;
    }
    int lastKey = 0;
    bool lastPressed = false;
    int calls = 0;
};

int main() {
    Hirari::Core::AudioProcessorGraph graph;
    graph.addNode(std::make_shared<Passthrough>());
    graph.prepare(48000.0, 64);
    assert(graph.isPrepared() && graph.validateGraph());
    assert(graph.getNodeNames().size() == 1);
    Hirari::Core::AudioBuffer buffer(2, 64);
    Hirari::Core::MidiBuffer midi;
    Hirari::DSP::ProcessContext context{};
    context.sampleRate = 48000.0;
    graph.process(buffer, midi, context);
    assert(graph.getProcessorFaultCount() == 0);

    // Non-finite samples must be removed at graph ingress even when the graph
    // is empty/bypassed. Otherwise an invalid device or bridge block can pass
    // straight through without reaching the per-node sanitizer.
    Hirari::Core::AudioBuffer ingress(2, 64);
    for (uint32_t c = 0; c < ingress.getNumChannels(); ++c) {
        float* samples = ingress.getWritePointer(c);
        for (uint32_t i = 0; i < ingress.getNumSamples(); ++i)
            samples[i] = (i % 2u == 0u)
                ? std::numeric_limits<float>::quiet_NaN()
                : std::numeric_limits<float>::infinity();
    }
    graph.process(ingress, midi, context);
    assert(graph.getSanitizedSampleCount() == 128);
    for (uint32_t c = 0; c < ingress.getNumChannels(); ++c)
        for (uint32_t i = 0; i < ingress.getNumSamples(); ++i)
            assert(std::isfinite(ingress.getReadPointer(c)[i]));

    assert(graph.replaceNode(0, std::make_shared<Passthrough>()));
    assert(graph.removeNode(0));
    assert(!graph.removeNode(0));

    Hirari::Core::AudioProcessorGraph sanitizer;
    sanitizer.addNode(std::make_shared<NaNProcessor>());
    sanitizer.prepare(48000.0, 64);
    sanitizer.process(buffer, midi, context);
    assert(sanitizer.getSanitizedSampleCount() == 128);
    for (uint32_t c = 0; c < 2; ++c)
        for (uint32_t i = 0; i < 64; ++i)
            assert(buffer.getReadPointer(c)[i] == 0.0f);

    auto source = std::make_shared<Source>();
    Hirari::Rendering::WaveformOverview overview(source);
    assert(overview.waitUntilReady(std::chrono::seconds(1)));
    Hirari::Rendering::WaveformOverview::LOD lod{};
    assert(overview.copyLOD(64, lod));
    assert(lod.minData.size() == 1 && lod.minData[0] == 0.25f);
    assert(lod.maxData[0] == 1.0f);
    std::vector<Hirari::Rendering::WaveformOverview::LOD> allLods;
    assert(overview.copyLODs(allLods) && allLods.size() == 3);
    Hirari::Rendering::WaveformOverview::LOD best{};
    assert(overview.copyBestLOD(1, best));
    assert(!overview.failed());

    auto& focus = Hirari::UI::Main::FocusManager::getInstance();
    focus.setFocus(static_cast<Hirari::UI::Main::FocusManager::EditorType>(999));
    assert(focus.getFocus() != static_cast<Hirari::UI::Main::FocusManager::EditorType>(999));
    KeyRecorder keys;
    focus.registerHandler(Hirari::UI::Main::FocusManager::EditorType::Arrangement, &keys);
    focus.setFocus(Hirari::UI::Main::FocusManager::EditorType::Arrangement);
    focus.handleKeyPress(65);
    focus.handleKeyPress(65);
    assert(keys.calls == 1 && keys.lastPressed && focus.isKeyDown(65));
    focus.handleKeyRelease(65);
    assert(keys.calls == 2 && !keys.lastPressed && !focus.isKeyDown(65));
    focus.unregisterHandler(Hirari::UI::Main::FocusManager::EditorType::Arrangement, &keys);
    return 0;
}
