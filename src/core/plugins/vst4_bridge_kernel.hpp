#pragma once
#include <vector>
#include <string>
#include <atomic>

#include <unordered_map>

namespace Aura::Core::Plugins {

struct VST4Parameter {
    uint32_t id;
    float value;
    float modulation; 
    std::atomic<float> target;
};

class VST4BridgeKernel {
public:
    VST4BridgeKernel() {}

    void addParameter(uint32_t id) {
        m_params.emplace(id, VST4Parameter{id, 0.0f, 0.0f, {0.0f}});
    }

    /**
     * @brief Pushes high-resolution parameter updates (O(1) dispatch).
     */
    void pushParameter(uint32_t id, float val) {
        auto it = m_params.find(id);
        if (it != m_params.end()) {
            it->second.target.store(val, std::memory_order_release);
        }
    }

    void syncParameters() {
        for (auto& [id, p] : m_params) {
            p.value = p.target.load(std::memory_order_acquire);
        }
    }

private:
    std::unordered_map<uint32_t, VST4Parameter> m_params;
};

} // namespace Aura::Core::Plugins
