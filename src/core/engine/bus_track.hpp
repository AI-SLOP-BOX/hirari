#pragma once

#include <atomic>
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>

#include "track.hpp"
#include "bus_system.hpp"

namespace Hirari::Core::Engine {

/** AUX/Bus track with an explicit pre-FX input and post-FX output boundary. */
class BusTrack : public Track {
public:
    BusTrack(uint32_t id, const std::string& name, uint32_t busId,
             SidechainManager* sidechainManager = nullptr,
             std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : Track(id, name, Track::Type::Bus, sidechainManager, std::move(layoutRevision)),
          m_busId(busId), m_trackState(hirari_bus_track_create(), &hirari_bus_track_destroy),
          m_busEffectChain(sidechainManager) {}

    void resolveBus(std::shared_ptr<::Hirari::Core::Engine::Bus> bus) noexcept {
        m_cachedBus = std::move(bus);
        m_cachedBusRaw = m_cachedBus.get();
    }
    void resolveBus(::Hirari::Core::Engine::Bus* bus) noexcept {
        m_cachedBus.reset();
        m_cachedBusRaw = bus;
    }
    uint32_t busId() const noexcept { return m_busId; }

    uint32_t getTotalLatencySamples() const noexcept override {
        const uint64_t total = static_cast<uint64_t>(Track::getTotalLatencySamples()) +
                               m_busEffectChain.getTotalLatencySamples();
        return static_cast<uint32_t>(std::min<uint64_t>(total, UINT32_MAX));
    }

    bool addPlugin(uint32_t pluginType) override {
        if (pluginType > 10) return false;
        const char* pluginPath = "Hirari/BusLimiter";
        if (pluginType == 1) pluginPath = "Hirari/Compressor";
        else if (pluginType == 2) pluginPath = "Hirari/Gate";
        else if (pluginType == 3) pluginPath = "Hirari/Saturation";
        else if (pluginType == 4) pluginPath = "Hirari/Transient";
        else if (pluginType == 5) pluginPath = "Hirari/DeEsser";
        else if (pluginType == 6) pluginPath = "Hirari/Delay";
        else if (pluginType == 7) pluginPath = "Hirari/Reverb";
        else if (pluginType == 8) pluginPath = "Hirari/DynamicEQ";
        else if (pluginType == 9) pluginPath = "Hirari/MidSide";
        else if (pluginType == 10) pluginPath = "Hirari/Width";
        auto processor = std::make_shared<Hirari::Core::Plugin::ExternalPluginHost>(
            pluginPath,
            Hirari::Core::Plugin::ExternalPluginHost::Format::Internal);
        if (!processor->isOperational()) return false;
        m_busEffectChain.addProcessor(std::move(processor));
        const uint32_t blockSize = getWorkBuffer(0).getNumSamples();
        if (blockSize > 0) m_busEffectChain.setSampleRate(getSampleRate(), blockSize);
        markProjectLayoutChanged();
        return true;
    }

    bool setPluginParameter(uint32_t pluginIndex, uint32_t parameterId, float value) override {
        const bool changed = m_busEffectChain.setParameter(pluginIndex, parameterId, value);
        if (changed) markProjectLayoutChanged();
        return changed;
    }

    // Bus plugins live in the dedicated bus chain; inheriting Track's
    // accessors would query the unused track chain and make UI/CLI reads
    // disagree with the value that was just written.
    float getPluginParameter(uint32_t pluginIndex, uint32_t parameterId) const noexcept override {
        const auto processor = m_busEffectChain.getProcessor(pluginIndex);
        return processor ? processor->getParameter(parameterId) : 0.0f;
    }

    uint32_t getPluginParameterCount(uint32_t pluginIndex) const noexcept override {
        const auto processor = m_busEffectChain.getProcessor(pluginIndex);
        return processor ? processor->getNumParameters() : 0;
    }

    void prepareToPlay(double sr, uint32_t blockSize) override {
        Track::prepareToPlay(sr, blockSize);
        m_busEffectChain.setSampleRate(sr, blockSize);
    }

    bool processAccumulated(uint32_t len, uint64_t playhead) override {
        if (!Track::processAccumulatedDeferredOutput(len, playhead)) return false;
        auto& work = getWorkBuffer(len);
        Hirari::Core::MidiBuffer midi;
        Hirari::DSP::ProcessContext context{};
        context.playhead = playhead;
        context.blockStart = playhead;
        context.blockEnd = playhead <= std::numeric_limits<uint64_t>::max() - len
            ? playhead + len
            : std::numeric_limits<uint64_t>::max();
        context.sampleRate = getSampleRate();
        context.bpm = getTransportBpm();
        context.blockSize = len;
        context.isPlaying = isTransportPlaying();
        context.numOutputChannels = work.getNumChannels();
        m_busEffectChain.syncToAudioThread();
        m_busEffectChain.process(work, midi, context, getId());
        finalizeTrackOutput(len, playhead);
        return true;
    }

    bool addPreFxInput(const AudioBuffer& input, uint32_t len, float gain) noexcept {
        if (!m_cachedBusRaw || input.getNumChannels() < 2 || len == 0 ||
            len > Bus::kMaxSamples || input.getNumSamples() < len) return false;
        return addPreFxInput(input.getReadPointer(0), input.getReadPointer(1), len, gain);
    }

    bool addPreFxInput(const float* left, const float* right,
                       uint32_t len, float gain) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > Bus::kMaxSamples) return false;
        return m_cachedBusRaw->addSamples(left, right, len, gain);
    }

    bool loadPreFxIntoWork(uint32_t len) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > Bus::kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        return m_cachedBusRaw->readPre(work.getWritePointer(0), work.getWritePointer(1), len);
    }

    void setReadPostFx(bool postFx) noexcept {
        hirari_bus_track_set_read_post_fx(m_trackState.get(), postFx);
    }

    void setInputGain(float gain) noexcept {
        hirari_bus_track_set_input_gain(m_trackState.get(), gain);
    }

    void setPhaseInverted(bool inverted) noexcept {
        hirari_bus_track_set_phase_inverted(m_trackState.get(), inverted);
    }

    // Copies the selected bus stage into the caller's output. This method is
    // allocation-free and is intended for the audio thread.
    void fetchAudio(float* l, float* r, uint64_t /*start*/, uint32_t len,
                    const ::Hirari::DSP::ProcessContext& /*context*/) {
        if (!l || !r || len == 0 || !m_cachedBusRaw) return;
        (void)hirari_bus_track_fetch_audio(
            m_trackState.get(), m_cachedBusRaw->nativeAudioState(), l, r, len);
    }

    // BusTrack processing boundary: pre-FX bus input is placed in the Track
    // work buffer, then the inherited Track chain processes it and the result
    // is published as the bus post-FX stage. No allocation is performed here.
    bool processBus(uint32_t len, uint64_t playhead) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > ::Hirari::Core::Engine::Bus::kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        float* left = work.getWritePointer(0);
        float* right = work.getWritePointer(1);
        fetchAudio(left, right, playhead, len, {});
        if (!processAccumulated(len, playhead)) return false;
        return m_cachedBusRaw->replacePostSamples(work.getReadPointer(0), work.getReadPointer(1), len);
    }

    bool publishPostFx(uint32_t len) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > Bus::kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        return m_cachedBusRaw->replacePostSamples(work.getReadPointer(0),
                                                  work.getReadPointer(1), len);
    }

private:
    uint32_t m_busId = 0;
    std::shared_ptr<::Hirari::Core::Engine::Bus> m_cachedBus;
    ::Hirari::Core::Engine::Bus* m_cachedBusRaw = nullptr;
    std::unique_ptr<void, decltype(&hirari_bus_track_destroy)> m_trackState{
        nullptr, &hirari_bus_track_destroy};
    Hirari::Core::EffectChain m_busEffectChain;
};

} // namespace Hirari::Core::Engine
