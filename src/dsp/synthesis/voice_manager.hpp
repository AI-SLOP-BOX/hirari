#include "ivoice.hpp"
#include "../../core/midi_buffer.hpp"
#include <memory>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>

namespace Aura::DSP::Synthesis {

/**
 * @class VoiceManager
 * @brief Global coordinator for polyphony and resource management.
 */
class VoiceManager {
public:
    static constexpr size_t kMaxGlobalVoices = 32;

    VoiceManager() = default;

    void prepareToPlay(double sampleRate, uint32_t maxBlockSize) noexcept {
        m_sampleRate = std::isfinite(sampleRate) && sampleRate > 1000.0 ? sampleRate : 44100.0;
        m_maxBlockSize = maxBlockSize;
    }

    /**
     * @brief Triggers a voice from the pool with zero allocation.
     */
    void triggerVoice(uint8_t note, uint8_t velocity) {
        if (m_voices.empty()) return;
        auto it = std::find_if(m_voices.begin(), m_voices.end(), [note](const auto& v) {
            return v && v->isActive() && v->getNote() == note;
        });
        if (it == m_voices.end()) {
            it = std::find_if(m_voices.begin(), m_voices.end(), [](const auto& v) { return v && !v->isActive(); });
        }
        if (it == m_voices.end()) it = m_voices.begin();
        if (*it) (*it)->noteOn(note, velocity);
    }

    void render(float* l, float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0) return;
        std::fill(l, l + numFrames, 0.0f);
        std::fill(r, r + numFrames, 0.0f);
        renderRange(l, r, numFrames);
    }

    void render(float* l, float* r, size_t numFrames, Core::MidiBuffer& midi) noexcept {
        if (!l || !r || numFrames == 0) return;
        std::fill(l, l + numFrames, 0.0f);
        std::fill(r, r + numFrames, 0.0f);
        midi.sort();

        size_t cursor = 0;
        for (const auto& event : midi) {
            const size_t eventOffset = std::min<size_t>(event.sampleOffset, numFrames);
            if (eventOffset > cursor) {
                renderRange(l + cursor, r + cursor, eventOffset - cursor);
                cursor = eventOffset;
            }
            if (event.size < 2) continue;
            const uint8_t status = static_cast<uint8_t>(event.data[0] & 0xF0u);
            const uint8_t note = event.data[1] & 0x7Fu;
            const uint8_t value = event.size >= 3 ? event.data[2] & 0x7Fu : 0;
            if (status == 0x90u && value != 0) triggerVoice(note, value);
            else if (status == 0x80u || (status == 0x90u && value == 0)) releaseVoice(note);
        }
        if (cursor < numFrames) renderRange(l + cursor, r + cursor, numFrames - cursor);
    }

    void releaseVoice(uint8_t note) {
        for (auto& voice : m_voices) if (voice && voice->isActive() && voice->getNote() == note) voice->noteOff(note);
    }


    void addVoice(std::unique_ptr<Core::DSP::Synthesis::IVoice> voice) {
        if (voice && m_voices.size() < kMaxGlobalVoices) m_voices.push_back(std::move(voice));
    }

private:
    void renderRange(float* l, float* r, size_t numFrames) noexcept {
        if (!l || !r || numFrames == 0) return;
        std::array<float, 4096> scratch{};
        size_t offset = 0;
        while (offset < numFrames) {
            const size_t count = std::min<size_t>(scratch.size(), numFrames - offset);
            std::fill(scratch.begin(), scratch.begin() + count, 0.0f);
            for (auto& voice : m_voices) {
                if (!voice || !voice->isActive()) continue;
                voice->process(scratch.data(), count);
            }
            for (size_t i = 0; i < count; ++i) {
                const float value = std::isfinite(scratch[i]) ? scratch[i] : 0.0f;
                l[offset + i] += value;
                r[offset + i] += value;
            }
            offset += count;
        }
    }

    std::vector<std::unique_ptr<Core::DSP::Synthesis::IVoice>> m_voices;
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 0;
};

} // namespace Aura::DSP::Synthesis
