#pragma once

#include "ivoice.hpp"
#include "../../core/midi_buffer.hpp"
#include <memory>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>

namespace Hirari::DSP::Synthesis {

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
        hirari_voice_manager_render_midi(
            midi.rustStateHandle(), this, l, r, numFrames,
            &VoiceManager::triggerVoiceCallback,
            &VoiceManager::releaseVoiceCallback,
            &VoiceManager::renderRangeCallback);
    }

    void releaseVoice(uint8_t note) {
        for (auto& voice : m_voices) if (voice && voice->isActive() && voice->getNote() == note) voice->noteOff(note);
    }


    void addVoice(std::unique_ptr<Core::DSP::Synthesis::IVoice> voice) {
        if (voice && m_voices.size() < kMaxGlobalVoices) m_voices.push_back(std::move(voice));
    }

private:
    static void triggerVoiceCallback(void* context, uint8_t note, uint8_t velocity) noexcept {
        if (context) static_cast<VoiceManager*>(context)->triggerVoice(note, velocity);
    }

    static void releaseVoiceCallback(void* context, uint8_t note) noexcept {
        if (context) static_cast<VoiceManager*>(context)->releaseVoice(note);
    }

    static void renderRangeCallback(void* context, float* left, float* right,
                                    size_t frames) noexcept {
        if (context) static_cast<VoiceManager*>(context)->renderRange(left, right, frames);
    }

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

} // namespace Hirari::DSP::Synthesis
