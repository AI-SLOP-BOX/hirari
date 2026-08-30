#pragma once

#include <atomic>
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>

#include "track.hpp"
#include "bus_system.hpp"

namespace Aura::Core::Engine {

/** AUX/Bus track with an explicit pre-FX input and post-FX output boundary. */
class BusTrack : public Track {
public:
    BusTrack(uint32_t id, const std::string& name, uint32_t busId,
             SidechainManager* sidechainManager = nullptr)
        : Track(id, name, Track::Type::Bus, sidechainManager),
          m_busId(busId), m_busEffectChain(sidechainManager) {}

    void resolveBus(std::shared_ptr<::Aura::Core::Engine::Bus> bus) noexcept {
        m_cachedBus = std::move(bus);
        m_cachedBusRaw = m_cachedBus.get();
    }
    void resolveBus(::Aura::Core::Engine::Bus* bus) noexcept {
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
        const char* pluginPath = "Aura/BusLimiter";
        if (pluginType == 1) pluginPath = "Aura/Compressor";
        else if (pluginType == 2) pluginPath = "Aura/Gate";
        else if (pluginType == 3) pluginPath = "Aura/Saturation";
        else if (pluginType == 4) pluginPath = "Aura/Transient";
        else if (pluginType == 5) pluginPath = "Aura/DeEsser";
        else if (pluginType == 6) pluginPath = "Aura/Delay";
        else if (pluginType == 7) pluginPath = "Aura/Reverb";
        else if (pluginType == 8) pluginPath = "Aura/DynamicEQ";
        else if (pluginType == 9) pluginPath = "Aura/MidSide";
        else if (pluginType == 10) pluginPath = "Aura/Width";
        auto processor = std::make_shared<Aura::Core::Plugin::ExternalPluginHost>(
            pluginPath,
            Aura::Core::Plugin::ExternalPluginHost::Format::Internal);
        if (!processor->isOperational()) return false;
        m_busEffectChain.addProcessor(std::move(processor));
        const uint32_t blockSize = getWorkBuffer(0).getNumSamples();
        if (blockSize > 0) m_busEffectChain.setSampleRate(getSampleRate(), blockSize);
        return true;
    }

    bool setPluginParameter(uint32_t pluginIndex, uint32_t parameterId, float value) override {
        return m_busEffectChain.setParameter(pluginIndex, parameterId, value);
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
        if (!Track::processAccumulated(len, playhead)) return false;
        auto& work = getWorkBuffer(len);
        Aura::Core::MidiBuffer midi;
        Aura::DSP::ProcessContext context{};
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
        applyPdcCompensation(work, len);
        return true;
    }

    bool addPreFxInput(const AudioBuffer& input, uint32_t len, float gain) noexcept {
        if (!m_cachedBusRaw || input.getNumChannels() < 2 || len == 0 ||
            len > Bus::kMaxSamples || input.getNumSamples() < len) return false;
        return m_cachedBusRaw->addSamples(input.getReadPointer(0), input.getReadPointer(1), len, gain);
    }

    bool loadPreFxIntoWork(uint32_t len) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > Bus::kMaxSamples || !canProcess(len)) return false;
        auto& work = getWorkBuffer(len);
        return m_cachedBusRaw->readPre(work.getWritePointer(0), work.getWritePointer(1), len);
    }

    void setReadPostFx(bool postFx) noexcept {
        m_readPostFx.store(postFx, std::memory_order_release);
    }

    void setInputGain(float gain) noexcept {
        m_inputGain.store(std::isfinite(gain) ? std::clamp(gain, 0.0f, 4.0f) : 1.0f,
                          std::memory_order_release);
    }

    void setPhaseInverted(bool inverted) noexcept {
        m_invertPhase.store(inverted, std::memory_order_release);
    }

    // Copies the selected bus stage into the caller's output. This method is
    // allocation-free and is intended for the audio thread.
    void fetchAudio(float* l, float* r, uint64_t /*start*/, uint32_t len,
                    const ::Aura::DSP::ProcessContext& /*context*/) {
        if (!l || !r || len == 0 || !m_cachedBusRaw) return;
        const bool post = m_readPostFx.load(std::memory_order_acquire);
        if (!(post ? m_cachedBusRaw->readPost(l, r, len) : m_cachedBusRaw->readPre(l, r, len))) {
            std::fill_n(l, len, 0.0f);
            std::fill_n(r, len, 0.0f);
            return;
        }
        const float gain = m_inputGain.load(std::memory_order_relaxed);
        const float sign = m_invertPhase.load(std::memory_order_relaxed) ? -1.0f : 1.0f;
        const float scale = gain * sign;
        for (uint32_t i = 0; i < len; ++i) {
            l[i] *= scale;
            r[i] *= scale;
        }
    }

    // BusTrack processing boundary: pre-FX bus input is placed in the Track
    // work buffer, then the inherited Track chain processes it and the result
    // is published as the bus post-FX stage. No allocation is performed here.
    bool processBus(uint32_t len, uint64_t playhead) noexcept {
        if (!m_cachedBusRaw || len == 0 || len > ::Aura::Core::Engine::Bus::kMaxSamples || !canProcess(len)) return false;
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
    std::shared_ptr<::Aura::Core::Engine::Bus> m_cachedBus;
    ::Aura::Core::Engine::Bus* m_cachedBusRaw = nullptr;
    std::atomic<float> m_inputGain{1.0f};
    std::atomic<bool> m_invertPhase{false};
    std::atomic<bool> m_readPostFx{true};
    Aura::Core::EffectChain m_busEffectChain;
};

} // namespace Aura::Core::Engine
