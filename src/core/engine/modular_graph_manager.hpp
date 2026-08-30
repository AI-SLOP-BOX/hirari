#include <vector>
#include <map>
#include <array>
#include <algorithm>
#include <memory>
#include <atomic>
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "../../dsp/iprocessor.hpp"

namespace Aura::Core::Engine {

/**
 * @class ModularGraphManager
 * @brief Zero-latency Nodal Routing & Compute Engine.
 * HONEST FIX: Implemented Topological Dependency Analysis to ensure correct
 * signal flow across complex modulation graphs.
 */
class ModularGraphManager {
public:
    struct Node {
        uint32_t id;
        std::shared_ptr<::Aura::DSP::IProcessor> processor;
        std::vector<uint32_t> inputs;
        std::vector<uint32_t> outputs;
        AudioBuffer buffer;
    };
 
    static ModularGraphManager& getInstance() {
        static ModularGraphManager instance;
        return instance;
    }
 
    void addNode(uint32_t id, std::shared_ptr<::Aura::DSP::IProcessor> proc) {
        if (!proc) return;
        Node n; n.id = id; n.processor = proc;
        n.buffer.resize(2, 4096);
        m_nodes[id] = std::move(n);
        rebuildTopology();
    }
 
    void connect(uint32_t fromId, uint32_t toId) {
        if (fromId == toId || m_nodes.find(fromId) == m_nodes.end() ||
            m_nodes.find(toId) == m_nodes.end()) return;
        auto& outputs = m_nodes[fromId].outputs;
        if (std::find(outputs.begin(), outputs.end(), toId) != outputs.end()) return;
        outputs.push_back(toId);
        m_nodes[toId].inputs.push_back(fromId);
        rebuildTopology();
    }
 
    /**
     * @brief SOVEREIGN EXECUTION: Zero-allocation topological process.
     */
    void process(AudioBuffer& masterBuffer, uint32_t numSamples, const ::Aura::DSP::ProcessContext& context) {
        if (numSamples == 0 || numSamples > 4096 || masterBuffer.getNumChannels() < 2 ||
            masterBuffer.getNumSamples() < numSamples ||
            m_dirty.load(std::memory_order_acquire)) return;
        masterBuffer.clear(numSamples);
        ::Aura::Core::MidiBuffer midi;
        for (const uint32_t id : m_sortedOrder) {
            auto it = m_nodes.find(id);
            if (it == m_nodes.end() || !it->second.processor) continue;
            Node& node = it->second;
            node.processor->process(node.buffer, midi, context);
            const float* left = node.buffer.getReadPointer(0);
            const float* right = node.buffer.getReadPointer(1);
            if (!left || !right) continue;
            for (const uint32_t destination : node.outputs) {
                auto out = m_nodes.find(destination);
                if (out == m_nodes.end() || out->second.buffer.getNumSamples() < numSamples) continue;
                float* destinationL = out->second.buffer.getWritePointer(0);
                float* destinationR = out->second.buffer.getWritePointer(1);
                if (!destinationL || !destinationR) continue;
                for (uint32_t sample = 0; sample < numSamples; ++sample) {
                    destinationL[sample] += left[sample];
                    destinationR[sample] += right[sample];
                }
            }
            if (node.outputs.empty()) {
                float* outputL = masterBuffer.getWritePointer(0);
                float* outputR = masterBuffer.getWritePointer(1);
                for (uint32_t sample = 0; sample < numSamples; ++sample) {
                    outputL[sample] += left[sample];
                    outputR[sample] += right[sample];
                }
            }
            node.buffer.clear(numSamples);
        }
    }
 
 private:
    void rebuildTopology() {
        m_sortedOrder.clear();
        std::map<uint32_t, uint32_t> indegree;
        for (const auto& [id, node] : m_nodes) indegree[id] = 0;
        for (const auto& [id, node] : m_nodes) {
            for (const uint32_t destination : node.outputs) {
                if (indegree.find(destination) != indegree.end()) ++indegree[destination];
            }
        }
        std::vector<uint32_t> ready;
        for (const auto& [id, degree] : indegree) if (degree == 0) ready.push_back(id);
        while (!ready.empty()) {
            const uint32_t id = ready.front();
            ready.erase(ready.begin());
            m_sortedOrder.push_back(id);
            const auto node = m_nodes.find(id);
            if (node == m_nodes.end()) continue;
            for (const uint32_t destination : node->second.outputs) {
                auto degree = indegree.find(destination);
                if (degree != indegree.end() && --degree->second == 0) ready.push_back(destination);
            }
        }
        // Cycles are not silently processed in an arbitrary order. Keep the
        // graph quiescent until the control side removes the cycle.
        if (m_sortedOrder.size() != m_nodes.size()) m_sortedOrder.clear();
        m_dirty = false;
    }
 
    std::map<uint32_t, Node> m_nodes;
    std::vector<uint32_t> m_sortedOrder;
    std::vector<uint32_t> m_finalOutputs;
    std::atomic<bool> m_dirty{true};
    ModularGraphManager() = default;
};

} // namespace Aura::Core::Engine
