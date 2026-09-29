#pragma once

#include <vector>
#include <memory>
#include <string>
#include <map>
#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <cmath>

#include "audio_buffer.hpp"
#include "midi_buffer.hpp"
#include "engine/bus_system.hpp"
#include "../dsp/iprocessor.hpp"
#include "concurrency/deferred_deleter.hpp"

namespace Hirari::Core {

/**
 * @class AudioProcessorGraph
 * @brief Orchestrates a dynamic chain of IProcessors with automatic Parallel Blending.
 */
class AudioProcessorGraph {
public:
    using IProcessor = DSP::IProcessor;

    AudioProcessorGraph() : m_runtimeState(hirari_audio_graph_runtime_create()) {}

    ~AudioProcessorGraph() { hirari_audio_graph_runtime_destroy(m_runtimeState); }

    void prepare(double sr, uint32_t bs) {
        if (!std::isfinite(sr) || sr <= 0.0 || bs == 0) {
            m_prepared = false;
            m_sampleRate = 0.0;
            m_maxBlockSize = 0;
            m_dryDelayStates.clear();
            hirari_audio_graph_runtime_reset_nodes(m_runtimeState, m_nodes.size());
            return;
        }
        m_sampleRate = sr;
        m_maxBlockSize = bs;
        m_dryBuffer.resize(2, bs); 
        m_dryDelayStates.clear();
        m_dryDelayStates.reserve(m_nodes.size());
        hirari_audio_graph_runtime_reset_nodes(m_runtimeState, m_nodes.size());
        for (size_t i = 0; i < m_nodes.size(); ++i) m_dryDelayStates.emplace_back();
        for (auto& node : m_nodes) {
            if (node) node->prepareToPlay(sr, bs);
        }
        m_prepared = std::all_of(m_dryDelayStates.begin(), m_dryDelayStates.end(),
            [](const DryDelayState& state) { return state.rustState != nullptr; });
    }

    void addNode(std::shared_ptr<IProcessor> node) {
        if (!node) return;
        m_nodes.push_back(node);
        hirari_audio_graph_runtime_append_node(m_runtimeState);
        if (m_sampleRate > 0) {
            node->prepareToPlay(m_sampleRate, m_maxBlockSize);
            m_dryDelayStates.emplace_back();
            if (m_prepared && !m_dryDelayStates.back().rustState) m_prepared = false;
        }
    }

    bool removeNode(size_t index) {
        if (index >= m_nodes.size()) return false;
        auto old = std::move(m_nodes[index]);
        m_nodes.erase(m_nodes.begin() + static_cast<std::ptrdiff_t>(index));
        if (index < m_dryDelayStates.size()) {
            m_dryDelayStates.erase(m_dryDelayStates.begin() + static_cast<std::ptrdiff_t>(index));
        }
        hirari_audio_graph_runtime_remove_node(m_runtimeState, index);
        Concurrency::DeferredDeleter::getInstance().push(std::move(old));
        if (m_nodes.empty()) m_prepared = false;
        return true;
    }

    bool replaceNode(size_t index, std::shared_ptr<IProcessor> replacement) {
        if (index >= m_nodes.size() || !replacement) return false;
        if (m_prepared) replacement->prepareToPlay(m_sampleRate, m_maxBlockSize);
        auto old = std::move(m_nodes[index]);
        m_nodes[index] = std::move(replacement);
        hirari_audio_graph_runtime_reset_node_fault(m_runtimeState, index);
        Concurrency::DeferredDeleter::getInstance().push(std::move(old));
        return true;
    }

    /**
     * @brief THE ENGINE ROOM: Processes the entire plugin graph.
     */
    void process(AudioBuffer& buffer, MidiBuffer& midi, const DSP::ProcessContext& context) {
        uint32_t numChannels = buffer.getNumChannels();
        uint32_t numSamples = buffer.getNumSamples();

        // Audio thread must never resize or allocate. A caller that requests a
        // block larger than prepare() supplied is rejected for this block.
        if (numChannels > 2 || m_dryBuffer.getNumChannels() < numChannels ||
            m_dryBuffer.getNumSamples() < numSamples) {
            hirari_audio_graph_runtime_note_rejected(m_runtimeState);
            buffer.clear();
            return;
        }
        if (numSamples == 0 || numChannels == 0 || !std::isfinite(context.sampleRate) ||
            context.sampleRate <= 0.0) {
            hirari_audio_graph_runtime_note_rejected(m_runtimeState);
            return;
        }

        float* channels[2]{};
        float* dryChannels[2]{};
        for (uint32_t c = 0; c < numChannels; ++c) {
            channels[c] = buffer.getWritePointer(c);
            dryChannels[c] = m_dryBuffer.getWritePointer(c);
        }
        ProcessCall call{this, &buffer, &midi, &context};
        if (!hirari_audio_graph_process_nodes(
                m_runtimeState, &call, channels, dryChannels, numChannels, numSamples,
                static_cast<uint32_t>(m_nodes.size()), &AudioProcessorGraph::getNodeInfo,
                &AudioProcessorGraph::processNode)) {
            buffer.clear();
            return;
        }
    }

    uint32_t getTotalLatency() const {
        return hirari_audio_graph_total_latency(
            const_cast<AudioProcessorGraph*>(this), static_cast<uint32_t>(m_nodes.size()),
            &AudioProcessorGraph::readNodeMetrics);
    }

    bool isPrepared() const noexcept {
        return m_prepared && m_sampleRate > 0.0 && std::isfinite(m_sampleRate) && m_maxBlockSize > 0 &&
               m_dryBuffer.getNumChannels() >= 2 && m_dryBuffer.getNumSamples() >= m_maxBlockSize &&
               m_dryDelayStates.size() == m_nodes.size() &&
               std::all_of(m_dryDelayStates.begin(), m_dryDelayStates.end(),
                   [](const DryDelayState& state) { return state.rustState != nullptr; });
    }

    bool validateGraph() const noexcept {
        return hirari_audio_graph_validate_nodes(
            const_cast<AudioProcessorGraph*>(this), static_cast<uint32_t>(m_nodes.size()),
            isPrepared(), &AudioProcessorGraph::readNodeMetrics);
    }

    size_t getNodeCount() const noexcept { return m_nodes.size(); }

    std::vector<std::string> getNodeNames() const {
        std::vector<std::string> names;
        names.reserve(m_nodes.size());
        for (const auto& node : m_nodes) {
            names.push_back(node ? node->getName() : "<invalid>");
        }
        return names;
    }

    uint64_t getSanitizedSampleCount() const noexcept {
        return hirari_audio_graph_runtime_metric(m_runtimeState, 0);
    }

    uint64_t getRejectedBlockCount() const noexcept {
        return hirari_audio_graph_runtime_metric(m_runtimeState, 1);
    }

    uint64_t getProcessorFaultCount() const noexcept {
        return hirari_audio_graph_runtime_metric(m_runtimeState, 2);
    }

    uint64_t getWatchdogTripCount() const noexcept {
        return hirari_audio_graph_runtime_metric(m_runtimeState, 3);
    }

    uint32_t getFaultedNodeCount() const noexcept {
        return hirari_audio_graph_runtime_faulted_node_count(m_runtimeState);
    }

    void reset() { for (auto& node : m_nodes) if (node) node->reset(); }
    void clear() { 
        // FIX: NEVER delete directly from the UI or Audio thread.
        // Use the DeferredDeleter to safely trash the shared pointers.
        for (auto& node : m_nodes) {
            Concurrency::DeferredDeleter::getInstance().push(std::move(node));
        }
        m_nodes.clear();
        m_dryDelayStates.clear();
        hirari_audio_graph_runtime_reset_nodes(m_runtimeState, 0);
        m_prepared = false;
    }

private:
    static bool readNodeMetrics(void* opaque, uint32_t index,
                                uint32_t* latency, float* mix) noexcept {
        auto* graph = static_cast<AudioProcessorGraph*>(opaque);
        if (!graph || !latency || !mix || index >= graph->m_nodes.size() ||
            !graph->m_nodes[index]) return false;
        *latency = graph->m_nodes[index]->getLatencySamples();
        *mix = graph->m_nodes[index]->getMix();
        return true;
    }

    struct ProcessCall {
        AudioProcessorGraph* graph;
        AudioBuffer* buffer;
        MidiBuffer* midi;
        const DSP::ProcessContext* context;
    };

    static bool getNodeInfo(void* opaque, uint32_t index, bool* bypassed,
                            float* mix, uint32_t* latency,
                            void** dryState) noexcept {
        auto* call = static_cast<ProcessCall*>(opaque);
        auto* graph = call ? call->graph : nullptr;
        if (!graph || !bypassed || !mix || !latency || !dryState ||
            index >= graph->m_nodes.size() || !graph->m_nodes[index]) return false;
        *bypassed = graph->m_nodes[index]->isBypassed();
        *mix = graph->m_nodes[index]->getMix();
        *latency = graph->m_nodes[index]->getLatencySamples();
        *dryState = index < graph->m_dryDelayStates.size()
            ? graph->m_dryDelayStates[index].rustState : nullptr;
        return true;
    }

    static uint8_t processNode(void* opaque, uint32_t index) noexcept {
        auto* call = static_cast<ProcessCall*>(opaque);
        auto* graph = call ? call->graph : nullptr;
        if (!graph || index >= graph->m_nodes.size() || !graph->m_nodes[index]) return 1;
        try {
            graph->m_nodes[index]->process(*call->buffer, *call->midi, *call->context);
        } catch (...) {
            return 1;
        }
        return graph->m_nodes[index]->takeWatchdogTrip() ? 2 : 0;
    }

    std::vector<std::shared_ptr<IProcessor>> m_nodes;
    AudioBuffer m_dryBuffer;
    struct DryDelayState {
        DryDelayState() : rustState(hirari_audio_graph_dry_state_create()) {}
        ~DryDelayState() { hirari_audio_graph_dry_state_destroy(rustState); }
        DryDelayState(const DryDelayState&) = delete;
        DryDelayState& operator=(const DryDelayState&) = delete;
        DryDelayState(DryDelayState&& other) noexcept : rustState(other.rustState) {
            other.rustState = nullptr;
        }
        DryDelayState& operator=(DryDelayState&& other) noexcept {
            if (this != &other) {
                hirari_audio_graph_dry_state_destroy(rustState);
                rustState = other.rustState;
                other.rustState = nullptr;
            }
            return *this;
        }
        void* rustState = nullptr;
    };
    std::vector<DryDelayState> m_dryDelayStates;
    void* m_runtimeState = nullptr;
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 512;
    bool m_prepared = false;
};

} // namespace Hirari::Core
