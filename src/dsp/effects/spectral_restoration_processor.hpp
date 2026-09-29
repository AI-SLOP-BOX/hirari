#pragma once

#include "../iprocessor.hpp"
#include "../analysis/spectral_processor.hpp"
#include "../../core/concurrency/status_queue.hpp"
#include <array>
#include <cstdio>
#include <cstring>

namespace Hirari::DSP::Effects {

/**
 * @class SpectralRestorationProcessor
 * @brief Industrial Surgical Repair Processor (Hirari Studio Pro).
 * Integrates the SpectralEditor kernel into the real-time processing chain.
 */
class SpectralRestorationProcessor : public IProcessor {
public:
    SpectralRestorationProcessor(double sr = 44100.0)
        : m_runtimeState(hirari_spectral_restoration_create(sr)) { reset(); }
    ~SpectralRestorationProcessor() override { hirari_spectral_restoration_destroy(m_runtimeState); }
    SpectralRestorationProcessor(const SpectralRestorationProcessor&) = delete;
    SpectralRestorationProcessor& operator=(const SpectralRestorationProcessor&) = delete;
    SpectralRestorationProcessor(SpectralRestorationProcessor&&) = delete;
    SpectralRestorationProcessor& operator=(SpectralRestorationProcessor&&) = delete;

    std::string getName() const override { return "Spectral Restoration"; }

    uint32_t getLatencySamples() const noexcept override { return hirari_spectral_restoration_latency(m_runtimeState); }
    uint32_t getTailSamples() const noexcept override { return hirari_spectral_restoration_tail(m_runtimeState); }

    // Drain per-channel OLA tails for offline export. Real-time processing
    // remains block-based and does not invoke this path.
    void flushOffline(Core::AudioBuffer& buffer, uint32_t offset, uint32_t samples) {
        if (buffer.getNumChannels() == 0 || samples == 0 || offset >= buffer.getNumSamples()) return;
        samples = std::min(samples, buffer.getNumSamples() - offset);
        std::array<float*, 12> channels{};
        const uint32_t count = std::min<uint32_t>(buffer.getNumChannels(), channels.size());
        for (uint32_t ch = 0; ch < count; ++ch) channels[ch] = buffer.getWritePointer(ch, offset);
        hirari_spectral_restoration_flush(m_runtimeState, channels.data(), count, samples);
    }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_spectral_restoration_set_parameter(m_runtimeState, id, value);
    }

    float getParameter(uint32_t id) const noexcept override {
        return hirari_spectral_restoration_get_parameter(m_runtimeState, id);
    }

    uint32_t getNumParameters() const noexcept override { return 2; }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id > 1) return false;
        out.minimum = 0.0f;
        out.maximum = 1.0f;
        out.stepped = id == 0;
        return true;
    }

    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Restoration Active" : (id == 1 ? "Denoise Threshold" : "");
        std::snprintf(outName, maxSize, "%s", name);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24u, 0u);
        hirari_spectral_restoration_write_state(
            m_runtimeState, isBypassed(), getSidechainBus(), state.data(), state.size());
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        bool bypassed = false;
        uint32_t sidechain = 0;
        if (!hirari_spectral_restoration_restore_state(
                m_runtimeState, state.data(), state.size(), &bypassed, &sidechain)) return false;
        setBypassed(bypassed);
        setSidechainBus(sidechain);
        return true;
    }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override {
        hirari_spectral_restoration_prepare(m_runtimeState, sr);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();

        std::array<float*, 12> channels{};
        const uint32_t count = std::min<uint32_t>(numChannels, channels.size());
        for (uint32_t ch = 0; ch < count; ++ch) channels[ch] = buffer.getWritePointer(ch);
        hirari_spectral_restoration_process(m_runtimeState, channels.data(), count, numSamples);
    }

    void setLearnMode(bool active) { hirari_spectral_restoration_set_learn(m_runtimeState, active); }
    void clearNoiseProfile() noexcept { hirari_spectral_restoration_clear_profile(m_runtimeState); }
    bool noiseProfileReady() const noexcept { return hirari_spectral_restoration_profile_ready(m_runtimeState); }
    void setRestorationActive(bool active) { setParameter(0, active ? 1.0f : 0.0f); }
    void setDenoiseThreshold(float t) { setParameter(1, std::isfinite(t) ? t / 8.0f : 0.125f); }
    
    void eraseHarmonics(float fund, float bw) {
        hirari_spectral_restoration_set_harmonics(m_runtimeState, fund, bw);
    }

    // Offline selected-region harmonic removal. The real-time editor keeps
    // its existing streaming request path; this overload applies the same
    // operation through the undoable spectral canvas processor.
    void eraseHarmonics(Core::AudioBuffer& buffer, float fundamentalHz,
                        const Analysis::SpectralProcessor::Rect& selection,
                        uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, sampleRate(), fundamentalHz,
                                     selection, harmonics, bandwidthHz);
    }

    // Offline/transport-safe surgical repairs exposed at the processor layer
    // so a UI or command client does not need to reach into individual editors.
    uint32_t removeClicks(Core::AudioBuffer& buffer, float threshold = 0.65f,
                          uint32_t radius = 8) {
        if (buffer.getNumChannels() == 0) return 0;
        return m_offlineProcessor.removeClicks(buffer, threshold, radius);
    }

    uint32_t removeClicks(Core::AudioBuffer& buffer,
                          const Analysis::SpectralProcessor::Rect& selection,
                          float threshold = 0.65f, uint32_t radius = 8) {
        if (buffer.getNumChannels() == 0) return 0;
        return m_offlineProcessor.removeClicks(buffer, sampleRate(), selection,
                                               threshold, radius);
    }

    uint32_t repairClipped(Core::AudioBuffer& buffer, float ceiling = 0.999f) {
        if (buffer.getNumChannels() == 0) return 0;
        return m_offlineProcessor.repairClipped(buffer, ceiling);
    }

    uint32_t repairClipped(Core::AudioBuffer& buffer,
                           const Analysis::SpectralProcessor::Rect& selection,
                           float ceiling = 0.999f) {
        if (buffer.getNumChannels() == 0) return 0;
        return m_offlineProcessor.repairClipped(buffer, sampleRate(), selection, ceiling);
    }

    void removeHum(Core::AudioBuffer& buffer, float fundamentalHz,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, sampleRate(), fundamentalHz, harmonics, bandwidthHz);
    }

    void removeHum(Core::AudioBuffer& buffer, float fundamentalHz,
                   const Analysis::SpectralProcessor::Rect& selection,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, sampleRate(), fundamentalHz,
                                     selection, harmonics, bandwidthHz);
    }

    void applySpectralGain(Core::AudioBuffer& buffer,
                           const Analysis::SpectralProcessor::Rect& target,
                           float gain) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.applySpectralGain(buffer, sampleRate(), target, gain);
    }

    void applySpectralGain(Core::AudioBuffer& buffer,
                           const std::vector<Analysis::SpectralProcessor::Rect>& regions,
                           float gain) {
        if (buffer.getNumChannels() == 0 || regions.empty()) return;
        m_offlineProcessor.applySpectralGain(buffer, sampleRate(), regions, gain);
    }

    void applySpectralMask(
        Core::AudioBuffer& buffer,
        const std::vector<Analysis::SpectralProcessor::RegionGain>& mask) {
        if (buffer.getNumChannels() == 0 || mask.empty()) return;
        m_offlineProcessor.applySpectralMask(buffer, sampleRate(), mask);
    }

    void healRegion(Core::AudioBuffer& buffer,
                    const Analysis::SpectralProcessor::Rect& target,
                    float amount = 1.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.healRegion(buffer, sampleRate(), target, amount);
    }

    void reduceNoise(Core::AudioBuffer& buffer, float amount = 1.0f,
                     float profileSeconds = 0.5f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.reduceNoise(buffer, sampleRate(), amount, profileSeconds);
    }

    void reduceNoise(Core::AudioBuffer& buffer,
                     const Analysis::SpectralProcessor::Rect& target,
                     float amount = 1.0f, float profileSeconds = 0.5f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.reduceNoise(buffer, sampleRate(), amount, profileSeconds, target);
    }

    void interpolateRegion(Core::AudioBuffer& buffer, const Analysis::SpectralProcessor::Rect& target,
                           float blend = 1.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.interpolateRegion(buffer, sampleRate(), target, blend);
    }

    // History controls for offline surgical edits. These remain separate from
    // the real-time editor state so transport processing is never interrupted.
    bool canUndoOffline() const noexcept { return m_offlineProcessor.canUndo(); }
    bool canRedoOffline() const noexcept { return m_offlineProcessor.canRedo(); }
    bool canUndoOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.canUndo(buffer); }
    bool canRedoOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.canRedo(buffer); }
    size_t undoDepthOffline() const noexcept { return m_offlineProcessor.undoDepth(); }
    size_t redoDepthOffline() const noexcept { return m_offlineProcessor.redoDepth(); }
    size_t undoDepthOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.undoDepth(buffer); }
    size_t redoDepthOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.redoDepth(buffer); }
    size_t historyBytesOffline() const noexcept { return m_offlineProcessor.historyBytes(); }
    const std::string& undoLabelOffline() const noexcept { return m_offlineProcessor.undoLabel(); }
    const std::string& redoLabelOffline() const noexcept { return m_offlineProcessor.redoLabel(); }
    const std::string& undoLabelOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.undoLabel(buffer); }
    const std::string& redoLabelOffline(const Core::AudioBuffer& buffer) const noexcept { return m_offlineProcessor.redoLabel(buffer); }
    bool undoOffline(Core::AudioBuffer& buffer) { return m_offlineProcessor.undo(buffer); }
    bool redoOffline(Core::AudioBuffer& buffer) { return m_offlineProcessor.redo(buffer); }
    void clearOfflineHistory() noexcept { m_offlineProcessor.clearHistory(); }
    void clearOfflineHistory(const Core::AudioBuffer& buffer) noexcept {
        m_offlineProcessor.clearHistory(buffer);
    }

    void reset() noexcept override {
        hirari_spectral_restoration_reset(m_runtimeState);
        m_offlineProcessor.clearHistory();
    }

private:
    double sampleRate() const noexcept { return hirari_spectral_restoration_sample_rate(m_runtimeState); }
    void* m_runtimeState = nullptr;
    Analysis::SpectralProcessor m_offlineProcessor;
};

} // namespace Hirari::DSP::Effects
