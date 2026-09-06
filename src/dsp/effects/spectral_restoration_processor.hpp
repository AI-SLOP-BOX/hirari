#pragma once

#include "../iprocessor.hpp"
#include "../analysis/spectral_editor.hpp"
#include "../analysis/spectral_processor.hpp"
#include "../../core/concurrency/status_queue.hpp"
#include <array>
#include <cstdio>
#include <cstring>

namespace Aura::DSP::Effects {

/**
 * @class SpectralRestorationProcessor
 * @brief Industrial Surgical Repair Processor (Aura Studio Pro).
 * Integrates the SpectralEditor kernel into the real-time processing chain.
 */
class SpectralRestorationProcessor : public IProcessor {
public:
    SpectralRestorationProcessor(double sr = 44100.0) 
        : m_sampleRate(std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0),
          m_editors{} {
        reset();
    }

    std::string getName() const override { return "Spectral Restoration"; }

    uint32_t getLatencySamples() const noexcept override { return m_editors.front().latencySamples(); }
    uint32_t getTailSamples() const noexcept override { return m_editors.front().tailSamples(); }

    // Drain per-channel OLA tails for offline export. Real-time processing
    // remains block-based and does not invoke this path.
    void flushOffline(Core::AudioBuffer& buffer, uint32_t offset, uint32_t samples) {
        if (buffer.getNumChannels() == 0 || samples == 0 || offset >= buffer.getNumSamples()) return;
        samples = std::min(samples, buffer.getNumSamples() - offset);
        const uint32_t count = std::min<uint32_t>(
            buffer.getNumChannels(), static_cast<uint32_t>(m_editors.size()));
        for (uint32_t ch = 0; ch < count; ++ch) {
            float* data = buffer.getWritePointer(ch, offset);
            if (data) m_editors[ch].flush(data, samples);
        }
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setRestorationActive(value >= 0.5f);
        else if (id == 1) setDenoiseThreshold(value * 8.0f);
    }

    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return m_restorationActive ? 1.0f : 0.0f;
        if (id == 1) return std::clamp(m_denoiseThreshold / 8.0f, 0.0f, 1.0f);
        return 0.0f;
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
        const uint32_t magic = 0x41555253u; // AURS
        const uint16_t version = 1u;
        const uint16_t flags = static_cast<uint16_t>(isBypassed() ? 1u : 0u);
        const uint32_t sidechain = getSidechainBus();
        const float values[2] = {getParameter(0), getParameter(1)};
        std::memcpy(state.data(), &magic, sizeof(magic));
        std::memcpy(state.data() + 4, &version, sizeof(version));
        std::memcpy(state.data() + 6, &flags, sizeof(flags));
        std::memcpy(state.data() + 8, &sidechain, sizeof(sidechain));
        std::memcpy(state.data() + 12, values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24u) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float values[2]{};
        std::memcpy(&magic, state.data(), sizeof(magic));
        std::memcpy(&version, state.data() + 4, sizeof(version));
        std::memcpy(&flags, state.data() + 6, sizeof(flags));
        std::memcpy(&sidechain, state.data() + 8, sizeof(sidechain));
        std::memcpy(values, state.data() + 12, sizeof(values));
        if (magic != 0x41555253u || version != 1u || (flags & ~1u) != 0u) return false;
        if (!std::isfinite(values[0]) || !std::isfinite(values[1]) ||
            values[0] < 0.0f || values[0] > 1.0f || values[1] < 0.0f || values[1] > 1.0f) return false;
        setBypassed((flags & 1u) != 0u);
        setSidechainBus(sidechain);
        setParameter(0, values[0]);
        setParameter(1, values[1]);
        return true;
    }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();

        // One independent editor is preallocated per immersive channel. Do
        // not reuse the last editor for channels beyond that capacity: its
        // OLA/noise-profile state would otherwise bleed between channels.
        const uint32_t processChannels = std::min<uint32_t>(
            numChannels, static_cast<uint32_t>(m_editors.size()));
        for (uint32_t ch = 0; ch < processChannels; ++ch) {
            float* data = buffer.getWritePointer(ch);
            if (data) m_editors[ch].process(data, data, numSamples);
        }
    }

    void setLearnMode(bool active) { for (auto& editor : m_editors) editor.setLearnMode(active); }
    void clearNoiseProfile() noexcept { for (auto& editor : m_editors) editor.clearNoiseProfile(); }
    bool noiseProfileReady() const noexcept { return m_editors.front().noiseProfileReady(); }
    void setRestorationActive(bool active) { m_restorationActive = active; for (auto& editor : m_editors) editor.setRestorationActive(active); }
    void setDenoiseThreshold(float t) { m_denoiseThreshold = std::isfinite(t) ? std::clamp(t, 0.0f, 8.0f) : 1.0f; for (auto& editor : m_editors) editor.setDenoiseThreshold(m_denoiseThreshold); }
    
    void eraseHarmonics(float fund, float bw) {
        for (auto& editor : m_editors) editor.requestEraseHarmonics(fund, (float)m_sampleRate, bw);
    }

    // Offline selected-region harmonic removal. The real-time editor keeps
    // its existing streaming request path; this overload applies the same
    // operation through the undoable spectral canvas processor.
    void eraseHarmonics(Core::AudioBuffer& buffer, float fundamentalHz,
                        const Analysis::SpectralProcessor::Rect& selection,
                        uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, m_sampleRate, fundamentalHz,
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
        return m_offlineProcessor.removeClicks(buffer, m_sampleRate, selection,
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
        return m_offlineProcessor.repairClipped(buffer, m_sampleRate, selection, ceiling);
    }

    void removeHum(Core::AudioBuffer& buffer, float fundamentalHz,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, m_sampleRate, fundamentalHz, harmonics, bandwidthHz);
    }

    void removeHum(Core::AudioBuffer& buffer, float fundamentalHz,
                   const Analysis::SpectralProcessor::Rect& selection,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.removeHum(buffer, m_sampleRate, fundamentalHz,
                                     selection, harmonics, bandwidthHz);
    }

    void applySpectralGain(Core::AudioBuffer& buffer,
                           const Analysis::SpectralProcessor::Rect& target,
                           float gain) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.applySpectralGain(buffer, m_sampleRate, target, gain);
    }

    void applySpectralGain(Core::AudioBuffer& buffer,
                           const std::vector<Analysis::SpectralProcessor::Rect>& regions,
                           float gain) {
        if (buffer.getNumChannels() == 0 || regions.empty()) return;
        m_offlineProcessor.applySpectralGain(buffer, m_sampleRate, regions, gain);
    }

    void applySpectralMask(
        Core::AudioBuffer& buffer,
        const std::vector<Analysis::SpectralProcessor::RegionGain>& mask) {
        if (buffer.getNumChannels() == 0 || mask.empty()) return;
        m_offlineProcessor.applySpectralMask(buffer, m_sampleRate, mask);
    }

    void healRegion(Core::AudioBuffer& buffer,
                    const Analysis::SpectralProcessor::Rect& target,
                    float amount = 1.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.healRegion(buffer, m_sampleRate, target, amount);
    }

    void reduceNoise(Core::AudioBuffer& buffer, float amount = 1.0f,
                     float profileSeconds = 0.5f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.reduceNoise(buffer, m_sampleRate, amount, profileSeconds);
    }

    void reduceNoise(Core::AudioBuffer& buffer,
                     const Analysis::SpectralProcessor::Rect& target,
                     float amount = 1.0f, float profileSeconds = 0.5f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.reduceNoise(buffer, m_sampleRate, amount, profileSeconds, target);
    }

    void interpolateRegion(Core::AudioBuffer& buffer, const Analysis::SpectralProcessor::Rect& target,
                           float blend = 1.0f) {
        if (buffer.getNumChannels() == 0) return;
        m_offlineProcessor.interpolateRegion(buffer, m_sampleRate, target, blend);
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
        m_restorationActive = false;
        m_denoiseThreshold = 1.0f;
        m_offlineProcessor.clearHistory();
        for (auto& editor : m_editors) {
            editor.reset();
            editor.setLearnMode(false);
            editor.setRestorationActive(false);
            editor.setDenoiseThreshold(m_denoiseThreshold);
        }
    }

private:
    double m_sampleRate;
    std::array<Analysis::SpectralEditor, 12> m_editors;
    Analysis::SpectralProcessor m_offlineProcessor;
    bool m_restorationActive = false;
    float m_denoiseThreshold = 1.0f;
};

} // namespace Aura::DSP::Effects
