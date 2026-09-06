#pragma once

#include <vector>
#include <map>
#include <algorithm>
#include <array>
#include <cstdio>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ScaleAssistant
 * @brief Professional MIDI Scale-Aware Snapping (Scale Quantize).
 * HONEST FIX: Transforms any incoming MIDI note to the nearest musically 
 * correct note within a chosen scale (e.g., C Major, D Minor).
 * Essential for modern producers who want to 'Never Miss a Note' during 
 * live performance or fast composition.
 */
class ScaleAssistant : public IProcessor {
public:
    enum class Scale { Chromatic, Major, Minor, Pentatonic };

    ScaleAssistant() : m_root(0), m_scale(Scale::Major) {
        for (auto& channel : m_noteMap) channel.fill(-1);
        updateActiveNotes();
    }

    std::string getName() const override { return "Scale Assistant"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setRoot(static_cast<int>(std::lround(std::clamp(value, 0.0f, 11.0f))));
        else if (id == 1) setScale(static_cast<Scale>(std::clamp(static_cast<int>(std::lround(value)), 0, 3)));
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return static_cast<float>(m_root);
        if (id == 1) return static_cast<float>(m_scale);
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id == 0) { out = {0.0f, 11.0f, true}; return true; }
        if (id == 1) { out = {0.0f, 3.0f, true}; return true; }
        return false;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        std::snprintf(outName, maxSize, "%s", id == 0 ? "Root Note" : (id == 1 ? "Scale" : ""));
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const uint32_t root = static_cast<uint32_t>(m_root); const uint32_t scale = static_cast<uint32_t>(m_scale);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4);
        std::memcpy(state.data()+16, &root, 4); std::memcpy(state.data()+20, &scale, 4); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0, root = 0, scale = 0; uint16_t version = 0, flags = 0; float mix = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(&root, state.data()+16, 4); std::memcpy(&scale, state.data()+20, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || root > 11 || scale > 3) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        setRoot(static_cast<int>(root)); setScale(static_cast<Scale>(scale)); return true;
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {}

    /**
     * @brief PROCESS: Snaps MIDI Note-Ons to the active scale.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)buffer; (void)context;
        if (m_bypassed) return;
        m_outputBuffer.clear();
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80) {
                m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                continue;
            }
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = event.data[0] & 0x0F;
            if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                const int remembered = m_noteMap[channel][event.data[1]];
                const uint8_t mapped = static_cast<uint8_t>(remembered >= 0 ? remembered : getNearestNote(event.data[1]));
                uint8_t data[3] = {event.data[0], mapped, static_cast<uint8_t>(event.size >= 3 ? event.data[2] : 0)};
                m_outputBuffer.addEvent(event.sampleOffset, data, 3, event.articulationId);
                m_noteMap[channel][event.data[1]] = -1;
            } else if (status == 0x90) {
                const uint8_t mapped = getNearestNote(event.data[1]);
                m_noteMap[channel][event.data[1]] = mapped;
                uint8_t data[3] = {event.data[0], mapped, event.data[2]};
                m_outputBuffer.addEvent(event.sampleOffset, data, 3, event.articulationId);
            } else {
                m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
            }
        }
        midi.clear();
        for (const auto& event : m_outputBuffer) midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        midi.sort();
    }


    void reset() noexcept override { for (auto& channel : m_noteMap) channel.fill(-1); }

    // Parameters
    void setRoot(int r) { m_root = ((r % 12) + 12) % 12; updateActiveNotes(); }
    void setScale(Scale s) { m_scale = s; updateActiveNotes(); }

private:
    uint8_t getNearestNote(uint8_t n) {
        int best = n;
        int minDist = 128;
        for (int octave = std::max(0, static_cast<int>(n / 12) - 1); octave <= std::min(10, static_cast<int>(n / 12) + 1); ++octave) {
            for (int active : m_activeNotes) {
                const int candidate = octave * 12 + active;
                const int dist = std::abs(candidate - static_cast<int>(n));
                if (candidate >= 0 && candidate <= 127 && dist < minDist) { minDist = dist; best = candidate; }
            }
        }
        return static_cast<uint8_t>(best);
    }

    void updateActiveNotes() {
        m_activeNotes.clear();
        std::vector<int> intervals;
        if (m_scale == Scale::Major) intervals = {0, 2, 4, 5, 7, 9, 11};
        else if (m_scale == Scale::Minor) intervals = {0, 2, 3, 5, 7, 8, 10};
        else if (m_scale == Scale::Pentatonic) intervals = {0, 2, 4, 7, 9};

        for (int i : intervals) m_activeNotes.push_back((m_root + i) % 12);
    }

    int m_root;
    Scale m_scale;
    std::vector<int> m_activeNotes;
    Core::MidiBuffer m_outputBuffer;
    std::array<std::array<int16_t, 128>, 16> m_noteMap{};
};

} // namespace Aura::DSP::Effects
