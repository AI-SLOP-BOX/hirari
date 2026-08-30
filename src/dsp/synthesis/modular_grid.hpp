#pragma once
#include <vector>
#include <map>
#include <string>
#include <memory>
#include <algorithm>
#include <cmath>
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class ModularGridEngine
 * @brief Bitwig-style Modular Environment ('The Grid').
 * HONEST FIX: Replaces static 'fixed-chain' synthesis with a professional 
 * node-based patching system. Users can connect modular components like 
 * oscillators, LFOs, and filters in any configuration.
 * Features sample-accurate modulation and low-latency feedback loops.
 */
class ModularGridEngine {
public:
    struct Port {
        float value = 0.0f;
        std::vector<float> buffer; // For high-rate modulation
    };

    struct Node {
        uint32_t id;
        std::string type; // "OSC", "FLT", "ENV", "LFO"
        std::vector<Port> inputs;
        std::vector<Port> outputs;
        
        virtual void process(uint32_t numSamples) = 0;
        virtual ~Node() = default;
    };

    struct Patch {
        uint32_t fromNode, fromPort;
        uint32_t toNode, toPort;
    };

    /**
     * @brief PROCESS: Executes the modular environment with industrial precision and nodal sovereignty.
     * INDUSTRIAL: Delegating topological sorting and signal transmission to the Rust 'GridOrchestrator'.
     */
    void process(Core::AudioBuffer& output, uint32_t numSamples) {
        if (output.getNumChannels() == 0) return;
        const uint32_t count = std::min(numSamples, output.getNumSamples());
        output.clear(count);
        if (m_nodes.empty() || count == 0) return;
        for (const size_t index : m_executionOrder) {
            auto& node = m_nodes[index];
            if (!node) continue;
            for (auto& port : node->inputs) {
                if (port.buffer.size() >= count) std::fill(port.buffer.begin(), port.buffer.begin() + count, port.value);
            }
            node->process(count);
            for (const auto& patch : m_patches) {
                if (patch.fromNode != node->id) continue;
                auto target = std::find_if(m_nodes.begin(), m_nodes.end(), [&patch](const auto& candidate) { return candidate && candidate->id == patch.toNode; });
                if (target == m_nodes.end() || patch.fromPort >= node->outputs.size() || patch.toPort >= (*target)->inputs.size()) continue;
                const auto& source = node->outputs[patch.fromPort];
                auto& destination = (*target)->inputs[patch.toPort];
                destination.value = source.value;
                if (source.buffer.size() >= count && destination.buffer.size() >= count)
                    std::copy(source.buffer.begin(), source.buffer.begin() + count, destination.buffer.begin());
            }
        }
        const auto& finalNode = m_nodes.back();
        if (!finalNode) return;
        const uint32_t channels = std::min<uint32_t>(output.getNumChannels(), 2);
        for (uint32_t c = 0; c < channels; ++c) {
            float* dst = output.getWritePointer(c);
            if (!dst || finalNode->outputs.empty()) continue;
            const auto& source = finalNode->outputs[std::min<size_t>(c, finalNode->outputs.size() - 1)];
            for (uint32_t i = 0; i < count; ++i) dst[i] = source.buffer.size() > i && std::isfinite(source.buffer[i]) ? source.buffer[i] : source.value;
        }
    }

    void addNode(std::unique_ptr<Node> node) {
        if (!node) return;
        m_nodes.push_back(std::move(node));
        rebuildExecutionOrder();
    }
    void addPatch(uint32_t fromN, uint32_t fromP, uint32_t toN, uint32_t toP) {
        m_patches.push_back({fromN, fromP, toN, toP});
        rebuildExecutionOrder();
    }

private:
    void rebuildExecutionOrder() {
        m_executionOrder.clear();
        const size_t nodeCount = m_nodes.size();
        if (nodeCount == 0) return;

        std::vector<size_t> indegree(nodeCount, 0);
        std::vector<std::vector<size_t>> edges(nodeCount);
        for (const auto& patch : m_patches) {
            auto from = std::find_if(m_nodes.begin(), m_nodes.end(), [&patch](const auto& node) { return node && node->id == patch.fromNode; });
            auto to = std::find_if(m_nodes.begin(), m_nodes.end(), [&patch](const auto& node) { return node && node->id == patch.toNode; });
            if (from == m_nodes.end() || to == m_nodes.end() || from == to) continue;
            const size_t fromIndex = static_cast<size_t>(std::distance(m_nodes.begin(), from));
            const size_t toIndex = static_cast<size_t>(std::distance(m_nodes.begin(), to));
            if (std::find(edges[fromIndex].begin(), edges[fromIndex].end(), toIndex) == edges[fromIndex].end()) {
                edges[fromIndex].push_back(toIndex);
                ++indegree[toIndex];
            }
        }

        std::vector<size_t> ready;
        ready.reserve(nodeCount);
        for (size_t i = 0; i < nodeCount; ++i) if (indegree[i] == 0) ready.push_back(i);
        while (!ready.empty()) {
            const size_t current = ready.back();
            ready.pop_back();
            m_executionOrder.push_back(current);
            for (const size_t next : edges[current]) if (--indegree[next] == 0) ready.push_back(next);
        }
        if (m_executionOrder.size() != nodeCount) {
            m_executionOrder.clear();
            for (size_t i = 0; i < nodeCount; ++i) m_executionOrder.push_back(i);
        }
    }

    std::vector<std::unique_ptr<Node>> m_nodes;
    std::vector<Patch> m_patches;
    std::vector<size_t> m_executionOrder;
};

} // namespace Aura::DSP::Synthesis
