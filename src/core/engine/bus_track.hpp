#pragma once

#include <atomic>
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>

#include "track.hpp"

namespace Hirari::Core::Engine {

/** AUX/Bus track with an explicit pre-FX input and post-FX output boundary. */
class BusTrack : public Track {
public:
    static constexpr uint32_t kMaxSamples = 8192;
    static constexpr uint32_t kMaxBuses = 128;

    BusTrack(uint32_t id, const std::string& name, uint32_t busId,
             void* sidechainEngine = nullptr,
             std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : Track(id, name, Track::Type::Bus, sidechainEngine, std::move(layoutRevision)),
          m_busId(busId), m_trackState(hirari_bus_track_create(), &hirari_bus_track_destroy),
          m_busEffectChain(sidechainEngine) {}

    void resolveBus(void* busState) noexcept { m_busState = busState; }
    uint32_t busId() const noexcept { return m_busId; }

    uint32_t getTotalLatencySamples() const noexcept override {
        const uint64_t total = static_cast<uint64_t>(Track::getTotalLatencySamples()) +
                               m_busEffectChain.getTotalLatencySamples();
        return static_cast<uint32_t>(std::min<uint64_t>(total, UINT32_MAX));
    }

    bool addPlugin(uint32_t pluginType) override {
        // A bus draws from the same Rust-owned catalog, except that the
        // limiter slot resolves to the bus limiter.
        const char* pluginPath = hirari_internal_plugin_path(pluginType, true);
        if (!pluginPath) return false;
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
        if (input.getNumChannels() < 2 || input.getNumSamples() < len) return false;
        return addPreFxInput(input.getReadPointer(0), input.getReadPointer(1), len, gain);
    }

    bool addPreFxInput(const float* left, const float* right,
                       uint32_t len, float gain) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples) return false;
        return hirari_bus_audio_accumulate(m_busState, left, right, len, gain);
    }

    bool loadPreFxIntoWork(uint32_t len) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        return hirari_bus_audio_read(m_busState, work.getWritePointer(0),
                                     work.getWritePointer(1), len, false);
    }

    void setPhaseInverted(bool inverted) noexcept {
        hirari_bus_track_set_phase_inverted(m_trackState.get(), inverted);
    }

    // Copies the selected bus stage into the caller's output. This method is
    // allocation-free and is intended for the audio thread.
    void fetchAudio(float* l, float* r, uint64_t /*start*/, uint32_t len,
                    const ::Hirari::DSP::ProcessContext& /*context*/) {
        if (!l || !r || len == 0 || !m_busState) return;
        (void)hirari_bus_track_fetch_audio(
            m_trackState.get(), m_busState, l, r, len);
    }

    // BusTrack processing boundary: pre-FX bus input is placed in the Track
    // work buffer, then the inherited Track chain processes it and the result
    // is published as the bus post-FX stage. No allocation is performed here.
    bool publishPostFx(uint32_t len) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        return hirari_bus_audio_replace_post(m_busState, work.getReadPointer(0),
                                             work.getReadPointer(1), len);
    }

private:
    uint32_t m_busId = 0;
    void* m_busState = nullptr;
    std::unique_ptr<void, decltype(&hirari_bus_track_destroy)> m_trackState{
        nullptr, &hirari_bus_track_destroy};
    Hirari::Core::EffectChain m_busEffectChain;
};

} // namespace Hirari::Core::Engine
