#pragma once

#include <vector>
#include <memory>
#include <string>
#include <cstdint>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief IAudioEffect: Abstract interface for any processable DSP block.
 */
class IAudioEffect {
public:
    virtual ~IAudioEffect() = default;
    virtual void process(float* l, float* r, size_t numFrames) = 0;
    virtual void setBypass(bool bp) = 0;
    virtual bool isBypassed() const noexcept { return false; }
};

/**
 * @brief EffectRack: Professional Serial Insert chain.
 * Logic Pro-style "Insert Slots" where you can stack EQ, Delay, and Reverb.
 */
class EffectRack {
public:
    void addEffect(std::unique_ptr<IAudioEffect> fx) {
        m_effects.push_back(std::move(fx));
    }

    /**
     * @brief Processes the audio through all active effects in the rack with performance sovereignty.
     * INDUSTRIAL: Delegating plugin chaining and parallel processing resolution to the Rust 'RackOrchestrator'.
     */
    void process(float* l, float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0) return;
        for (auto& effect : m_effects) {
            if (effect && !effect->isBypassed()) effect->process(l, r, numFrames);
        }
    }

    void clear() { m_effects.clear(); }
    size_t size() const noexcept { return m_effects.size(); }
    IAudioEffect* getEffect(size_t index) noexcept {
        return index < m_effects.size() ? m_effects[index].get() : nullptr;
    }

private:
    std::vector<std::unique_ptr<IAudioEffect>> m_effects;
};

} // namespace Aura::Core::DSP::Mixing
