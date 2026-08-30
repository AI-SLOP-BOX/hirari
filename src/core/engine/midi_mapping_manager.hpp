#pragma once
#include <algorithm>
#include <atomic>
#include <cmath>
#include <memory>
#include <vector>
#include <mutex>
#include <cstdint>
#include "param_tree.hpp"

namespace Aura::Core::Engine {

/**
 * @class MidiMappingManager
 * @brief Professional Hardware-to-Param Mapping Engine.
 * HONEST FIX: Implements 'MIDI Learn' functionality.
 * Allows any internal DAW parameter (Volume, Cutoff, Macro) to be 
 * instantly mapped to a physical MIDI controller CC message.
 */
class MidiMappingManager {
public:
    static MidiMappingManager& getInstance() { static MidiMappingManager i; return i; }

    struct Mapping {
        uint32_t parameterId = 0;
        uint8_t channel = 16; // 16 = omni
        uint8_t cc = 0;
        float minimum = 0.0f;
        float maximum = 1.0f;
        float curve = 0.0f;
        bool pickup = false;
        std::atomic<float>* target = nullptr;
        std::shared_ptr<std::atomic<bool>> acquired;
    };

    void learn(uint32_t paramId) {
        m_pendingParameter.store(paramId, std::memory_order_release);
    }

    // Control-thread API. The published snapshot is immutable for readers,
    // so MIDI input never waits for a map mutation.
    bool bind(uint32_t parameterId, uint8_t channel, uint8_t cc,
              std::atomic<float>* target, float minimum = 0.0f,
              float maximum = 1.0f, float curve = 0.0f, bool pickup = false) {
        if (target == nullptr || channel > 16 || cc > 127 ||
            !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
            !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) return false;
        std::lock_guard<std::mutex> lock(m_writeMutex);
        auto next = std::make_shared<std::vector<Mapping>>(*std::atomic_load(&m_snapshot));
        next->erase(std::remove_if(next->begin(), next->end(),
            [channel, cc](const Mapping& item) {
                return item.channel == channel && item.cc == cc;
            }), next->end());
        next->push_back({parameterId, channel, cc, minimum, maximum, curve, pickup,
                         target, std::make_shared<std::atomic<bool>>(!pickup)});
        std::atomic_store(&m_snapshot, std::shared_ptr<const std::vector<Mapping>>(next));
        return true;
    }

    bool unbind(uint8_t channel, uint8_t cc) {
        if (channel > 16 || cc > 127) return false;
        std::lock_guard<std::mutex> lock(m_writeMutex);
        auto next = std::make_shared<std::vector<Mapping>>(*std::atomic_load(&m_snapshot));
        const auto oldSize = next->size();
        next->erase(std::remove_if(next->begin(), next->end(),
            [channel, cc](const Mapping& item) {
                return item.channel == channel && item.cc == cc;
            }), next->end());
        std::atomic_store(&m_snapshot, std::shared_ptr<const std::vector<Mapping>>(next));
        return oldSize != next->size();
    }

    void handleCC(uint8_t channel, uint8_t cc, uint8_t value) noexcept {
        if (channel > 15 || cc > 127) return;
        const auto mappings = std::atomic_load(&m_snapshot);
        const float normalized = static_cast<float>(value) / 127.0f;
        for (const auto& mapping : *mappings) {
            if (mapping.cc != cc ||
                (mapping.channel != 16 && mapping.channel != channel) ||
                mapping.target == nullptr) continue;
            const float current = std::clamp(mapping.target->load(std::memory_order_relaxed),
                                             mapping.minimum, mapping.maximum);
            if (mapping.pickup && mapping.acquired != nullptr &&
                !mapping.acquired->load(std::memory_order_relaxed)) {
                const float span = std::max(0.0001f, mapping.maximum - mapping.minimum);
                if (std::abs(current - (mapping.minimum + span * normalized)) > span * 0.02f)
                    continue;
                mapping.acquired->store(true, std::memory_order_relaxed);
            }
            const float shaped = mapping.curve == 0.0f ? normalized :
                (mapping.curve > 0.0f
                    ? std::pow(normalized, 1.0f + mapping.curve * 3.0f)
                    : 1.0f - std::pow(1.0f - normalized, 1.0f - mapping.curve * 3.0f));
            mapping.target->store(mapping.minimum +
                (mapping.maximum - mapping.minimum) * std::clamp(shaped, 0.0f, 1.0f),
                std::memory_order_relaxed);
        }
    }

private:
    MidiMappingManager() = default;
    std::shared_ptr<const std::vector<Mapping>> m_snapshot =
        std::make_shared<const std::vector<Mapping>>();
    std::mutex m_writeMutex;
    std::atomic<uint32_t> m_pendingParameter{0};
};


} // namespace Aura::Core::Engine
