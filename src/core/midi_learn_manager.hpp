#pragma once

#include <cstdint>
#include <vector>
#include <atomic>
#include <memory>
#include <mutex>
#include <algorithm>
#include <cmath>

namespace Aura::Core {

/**
 * @brief MidiLearnManager: Maps MIDI CC messages to synth/mixer parameters.
 * Addresses the "missing controller integration" from the review.
 */
class MidiLearnManager {
public:
    struct Mapping {
        // 7-bit CCs occupy 0..127; paired CC/NRPN mappings may use the full
        // 14-bit controller namespace without truncation.
        uint16_t cc;
        std::atomic<float>* parameter;
        uint8_t channel = 0; // 0..15, 16 means omni
        float minimum = 0.0f;
        float maximum = 1.0f;
        float curve = 0.0f; // negative = soft, positive = hard
        bool pickup = false;
        bool is14Bit = false;
        // Mutable runtime state lives outside the immutable mapping snapshot.
        // This keeps audio-thread reads lock-free while still allowing pickup
        // to latch after the first matching controller position.
        std::shared_ptr<std::atomic<bool>> pickedUp;
    };

    /**
     * @brief Adds a new MIDI mapping.
     */
    void addMapping(uint8_t cc, std::atomic<float>* param) {
        addMappingAdvanced(cc, 16, param, 0.0f, 1.0f, 0.0f, false);
    }

    void addMappingAdvanced(uint8_t cc, uint8_t channel, std::atomic<float>* param,
                            float minimum, float maximum, float curve, bool pickup) {
        if (cc > 127 || channel > 16 || param == nullptr ||
            !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
            !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) return;
        std::lock_guard<std::mutex> lock(m_writeMutex);
        auto next = std::make_shared<std::vector<Mapping>>(*std::atomic_load(&m_snapshot));
        next->erase(std::remove_if(next->begin(), next->end(),
            [cc, channel](const Mapping& mapping) {
                return mapping.cc == cc && mapping.channel == channel;
            }), next->end());
        next->push_back({cc, param, channel, minimum, maximum, curve, pickup, false,
                         std::make_shared<std::atomic<bool>>(!pickup)});
        std::atomic_store(&m_snapshot, std::shared_ptr<const std::vector<Mapping>>(next));
    }

    void addMapping14Bit(uint16_t controller, uint8_t channel,
                         std::atomic<float>* param, float minimum,
                         float maximum, float curve, bool pickup) {
        if (controller > 16383 || channel > 16 || param == nullptr ||
            !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
            !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) return;
        std::lock_guard<std::mutex> lock(m_writeMutex);
        auto next = std::make_shared<std::vector<Mapping>>(*std::atomic_load(&m_snapshot));
        next->erase(std::remove_if(next->begin(), next->end(),
            [controller, channel](const Mapping& mapping) {
                return mapping.cc == controller && mapping.channel == channel;
            }), next->end());
        next->push_back({controller, param, channel, minimum, maximum, curve, pickup, true,
                         std::make_shared<std::atomic<bool>>(!pickup)});
        std::atomic_store(&m_snapshot, std::shared_ptr<const std::vector<Mapping>>(next));
    }

    void removeMapping(uint8_t cc, uint8_t channel = 16) {
        if (cc > 127 || channel > 16) return;
        std::lock_guard<std::mutex> lock(m_writeMutex);
        auto next = std::make_shared<std::vector<Mapping>>(*std::atomic_load(&m_snapshot));
        next->erase(std::remove_if(next->begin(), next->end(),
            [cc, channel](const Mapping& mapping) {
                return mapping.cc == cc && mapping.channel == channel;
            }), next->end());
        std::atomic_store(&m_snapshot, std::shared_ptr<const std::vector<Mapping>>(next));
    }

    /**
     * @brief Processes an incoming MIDI CC message and updates the linked parameter.
     */
    void handleMidiCC(uint8_t channel, uint8_t cc, uint8_t value) noexcept {
        if (channel > 15 || cc > 127) return;
        const auto mappings = std::atomic_load(&m_snapshot);
        const float normalized = static_cast<float>(value) / 127.0f;
        for (auto& mapping : *mappings) {
            if (mapping.is14Bit || mapping.cc != cc ||
                (mapping.channel != 16 && mapping.channel != channel) ||
                mapping.parameter == nullptr) continue;
            const float current = std::clamp(mapping.parameter->load(std::memory_order_relaxed),
                                             mapping.minimum, mapping.maximum);
            if (mapping.pickup && mapping.pickedUp != nullptr &&
                !mapping.pickedUp->load(std::memory_order_relaxed)) {
                const float span = std::max(0.0001f, mapping.maximum - mapping.minimum);
                const float target = mapping.minimum + span * normalized;
                if (std::abs(current - target) > span * 0.02f) continue;
                mapping.pickedUp->store(true, std::memory_order_relaxed);
            }
            const float shaped = mapping.curve == 0.0f
                ? normalized
                : (mapping.curve > 0.0f
                    ? std::pow(normalized, 1.0f + mapping.curve * 3.0f)
                    : 1.0f - std::pow(1.0f - normalized, 1.0f - mapping.curve * 3.0f));
            mapping.parameter->store(mapping.minimum +
                (mapping.maximum - mapping.minimum) * std::clamp(shaped, 0.0f, 1.0f),
                std::memory_order_relaxed);
        }
    }

    void handleMidiCC14(uint8_t channel, uint16_t controller, uint16_t value) noexcept {
        if (channel > 15 || controller > 16383 || value > 16383) return;
        const auto mappings = std::atomic_load(&m_snapshot);
        const float normalized = static_cast<float>(value) / 16383.0f;
        for (auto& mapping : *mappings) {
            if (!mapping.is14Bit || mapping.cc != controller ||
                (mapping.channel != 16 && mapping.channel != channel) ||
                mapping.parameter == nullptr) continue;
            const float current = std::clamp(mapping.parameter->load(std::memory_order_relaxed),
                                             mapping.minimum, mapping.maximum);
            if (mapping.pickup && mapping.pickedUp != nullptr &&
                !mapping.pickedUp->load(std::memory_order_relaxed)) {
                const float span = std::max(0.0001f, mapping.maximum - mapping.minimum);
                const float target = mapping.minimum + span * normalized;
                if (std::abs(current - target) > span * 0.02f) continue;
                mapping.pickedUp->store(true, std::memory_order_relaxed);
            }
            const float shaped = mapping.curve == 0.0f
                ? normalized
                : (mapping.curve > 0.0f
                    ? std::pow(normalized, 1.0f + mapping.curve * 3.0f)
                    : 1.0f - std::pow(1.0f - normalized, 1.0f - mapping.curve * 3.0f));
            mapping.parameter->store(mapping.minimum +
                (mapping.maximum - mapping.minimum) * std::clamp(shaped, 0.0f, 1.0f),
                std::memory_order_relaxed);
        }
    }

    // Compatibility form for older device bridges that do not report MIDI
    // channel information.
    void handleMidiCC(uint8_t cc, uint8_t value) noexcept { handleMidiCC(0, cc, value); }

private:
    std::shared_ptr<const std::vector<Mapping>> m_snapshot =
        std::make_shared<const std::vector<Mapping>>();
    std::mutex m_writeMutex;
};

} // namespace Aura::Core
