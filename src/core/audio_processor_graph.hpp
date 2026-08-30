#pragma once

#include <vector>
#include <memory>
#include <string>
#include <map>
#include <algorithm>
#include <array>
#include <cstdint>
#include <limits>
#include <cstring>
#include <cmath>
#include <atomic>

#include "audio_buffer.hpp"
#include "midi_buffer.hpp"
#include "engine/bus_system.hpp"
#include "../dsp/iprocessor.hpp"
#include "../dsp/effects/delay_line.hpp"
#include "concurrency/deferred_deleter.hpp"

namespace Aura::Core {

/**
 * @class AudioProcessorGraph
 * @brief Orchestrates a dynamic chain of IProcessors with automatic Parallel Blending.
 */
class AudioProcessorGraph {
public:
    using IProcessor = DSP::IProcessor;

    AudioProcessorGraph() = default;

    void prepare(double sr, uint32_t bs) {
        if (!std::isfinite(sr) || sr <= 0.0 || bs == 0) {
            m_prepared = false;
            m_sampleRate = 0.0;
            m_maxBlockSize = 0;
            m_dryDelayStates.clear();
            return;
        }
        m_sampleRate = sr;
        m_maxBlockSize = bs;
        m_dryBuffer.resize(2, bs); 
        m_dryDelayStates.clear();
        m_dryDelayStates.reserve(m_nodes.size());
        m_faultedNodes.assign(m_nodes.size(), false);
        for (size_t i = 0; i < m_nodes.size(); ++i) m_dryDelayStates.emplace_back();
        for (auto& node : m_nodes) {
            if (node) node->prepareToPlay(sr, bs);
        }
        m_prepared = true;
    }

    void addNode(std::shared_ptr<IProcessor> node) {
        if (!node) return;
        m_nodes.push_back(node);
        if (m_sampleRate > 0) {
            node->prepareToPlay(m_sampleRate, m_maxBlockSize);
            m_dryDelayStates.emplace_back();
            m_faultedNodes.push_back(false);
        }
    }

    bool removeNode(size_t index) {
        if (index >= m_nodes.size()) return false;
        auto old = std::move(m_nodes[index]);
        m_nodes.erase(m_nodes.begin() + static_cast<std::ptrdiff_t>(index));
        if (index < m_dryDelayStates.size()) {
            m_dryDelayStates.erase(m_dryDelayStates.begin() + static_cast<std::ptrdiff_t>(index));
        }
        if (index < m_faultedNodes.size()) {
            m_faultedNodes.erase(m_faultedNodes.begin() + static_cast<std::ptrdiff_t>(index));
        }
        Concurrency::DeferredDeleter::getInstance().push(std::move(old));
        if (m_nodes.empty()) m_prepared = false;
        return true;
    }

    bool replaceNode(size_t index, std::shared_ptr<IProcessor> replacement) {
        if (index >= m_nodes.size() || !replacement) return false;
        if (m_prepared) replacement->prepareToPlay(m_sampleRate, m_maxBlockSize);
        auto old = std::move(m_nodes[index]);
        m_nodes[index] = std::move(replacement);
        if (index < m_faultedNodes.size()) m_faultedNodes[index] = false;
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
        if (m_dryBuffer.getNumChannels() < numChannels || m_dryBuffer.getNumSamples() < numSamples) {
            m_rejectedBlocks.fetch_add(1, std::memory_order_relaxed);
            buffer.clear();
            return;
        }
        if (numSamples == 0 || numChannels == 0 || !std::isfinite(context.sampleRate) ||
            context.sampleRate <= 0.0) {
            m_rejectedBlocks.fetch_add(1, std::memory_order_relaxed);
            return;
        }

        // Sanitize at ingress, not only after a processor runs. Bypassed
        // nodes and an empty graph otherwise allow NaN/Inf from a device or
        // plugin boundary to pass directly to the output unchanged.
        sanitizeBuffer(buffer);

        size_t nodeIndex = 0;
        for (auto& node : m_nodes) {
            if (!node) {
                m_rejectedBlocks.fetch_add(1, std::memory_order_relaxed);
                buffer.clear();
                return;
            }
            if (nodeIndex < m_faultedNodes.size() && m_faultedNodes[nodeIndex]) {
                ++nodeIndex;
                continue;
            }
            if (node->isBypassed()) {
                ++nodeIndex;
                continue;
            }

            float mix = node->getMix();
            uint32_t latency = node->getLatencySamples();
            if (!std::isfinite(mix) || mix < 0.0f || mix > 1.0f || latency > 65535u) {
                m_rejectedBlocks.fetch_add(1, std::memory_order_relaxed);
                buffer.clear();
                return;
            }
            
            if (mix < 1.0f) {
                // Delay lines are prepared up front; never grow them on the
                // real-time thread.
                if (nodeIndex >= m_dryDelayStates.size()) {
                    ++nodeIndex;
                    continue;
                }

                auto& delay = m_dryDelayStates[nodeIndex];
                if (latency != delay.activeLatency && latency != delay.pendingLatency) {
                    delay.pendingLatency = latency;
                    delay.transitionRemaining = kLatencyCrossfadeSamples;
                }

                for (uint32_t c = 0; c < numChannels; ++c) {
                    float* dry = m_dryBuffer.getWritePointer(c);
                    const float* src = buffer.getReadPointer(c);

                    // The prepared dry-delay bank is stereo. For wider buses,
                    // preserve a deterministic dry path instead of mixing a
                    // stale channel from a previous node/block.
                    if (c >= delay.lines.size()) {
                        std::memcpy(dry, src, numSamples * sizeof(float));
                        continue;
                    }
                    if (latency > 0 || delay.activeLatency > 0 || delay.pendingLatency > 0) {
                        for (uint32_t s = 0; s < numSamples; ++s) {
                            const float oldValue = delay.lines[c][delay.activeLine].process(
                                src[s], delay.activeLatency);
                            const float newValue = delay.lines[c][1 - delay.activeLine].process(
                                src[s], delay.pendingLatency);
                            if (delay.transitionRemaining > 0) {
                                const float progress = 1.0f -
                                    static_cast<float>(delay.transitionRemaining) /
                                    static_cast<float>(kLatencyCrossfadeSamples);
                                dry[s] = oldValue * (1.0f - progress) + newValue * progress;
                                if (c + 1 == numChannels && --delay.transitionRemaining == 0) {
                                    delay.activeLine = 1 - delay.activeLine;
                                    delay.activeLatency = delay.pendingLatency;
                                }
                            } else {
                                dry[s] = newValue;
                            }
                        }
                    } else {
                        std::memcpy(dry, src, numSamples * sizeof(float));
                    }
                }
            }

            try {
                node->process(buffer, midi, context);
            } catch (...) {
                // Third-party processors must never take down the audio graph.
                // The process contract is noexcept, but this guard protects
                // the host when a legacy implementation violates it.
                m_processorFaults.fetch_add(1, std::memory_order_relaxed);
                if (nodeIndex < m_faultedNodes.size()) m_faultedNodes[nodeIndex] = true;
                buffer.clear();
                ++nodeIndex;
                continue;
            }

            sanitizeBuffer(buffer);

            if (mix < 1.0f) {
                float invMix = 1.0f - mix;
                for (uint32_t c = 0; c < numChannels; ++c) {
                    float* wet = buffer.getWritePointer(c);
                    const float* dry = m_dryBuffer.getReadPointer(c);

                    // --- HONEST FIX: OPTIMIZED BLENDING ---
                    for (uint32_t s = 0; s < numSamples; ++s) {
                        wet[s] = (wet[s] * mix) + (dry[s] * invMix);
                    }
                }
            }
            sanitizeBuffer(buffer);
            ++nodeIndex;
        }
    }

    uint32_t getTotalLatency() const {
        uint32_t total = 0;
        for (const auto& node : m_nodes) {
            if (!node) continue;
            const uint32_t latency = node->getLatencySamples();
            total = (std::numeric_limits<uint32_t>::max() - total < latency)
                ? std::numeric_limits<uint32_t>::max() : total + latency;
        }
        return total;
    }

    bool isPrepared() const noexcept {
        return m_prepared && m_sampleRate > 0.0 && std::isfinite(m_sampleRate) && m_maxBlockSize > 0 &&
               m_dryBuffer.getNumChannels() >= 2 && m_dryBuffer.getNumSamples() >= m_maxBlockSize &&
               m_dryDelayStates.size() == m_nodes.size();
    }

    bool validateGraph() const noexcept {
        if (!isPrepared()) return false;
        for (const auto& node : m_nodes) {
            if (!node || node->getLatencySamples() > 65535u ||
                !std::isfinite(node->getMix()) || node->getMix() < 0.0f || node->getMix() > 1.0f) {
                return false;
            }
        }
        return true;
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
        return m_sanitizedSamples.load(std::memory_order_relaxed);
    }

    uint64_t getRejectedBlockCount() const noexcept {
        return m_rejectedBlocks.load(std::memory_order_relaxed);
    }

    uint64_t getProcessorFaultCount() const noexcept {
        return m_processorFaults.load(std::memory_order_relaxed);
    }

    uint32_t getFaultedNodeCount() const noexcept {
        uint32_t count = 0;
        for (const bool faulted : m_faultedNodes) if (faulted) ++count;
        return count;
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
        m_faultedNodes.clear();
        m_prepared = false;
    }

private:
    void sanitizeBuffer(AudioBuffer& buffer) noexcept {
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            float* samples = buffer.getWritePointer(channel);
            if (!samples) continue;
            for (uint32_t index = 0; index < buffer.getNumSamples(); ++index) {
                if (!std::isfinite(samples[index])) {
                    samples[index] = 0.0f;
                    m_sanitizedSamples.fetch_add(1, std::memory_order_relaxed);
                }
            }
        }
    }

    std::vector<std::shared_ptr<IProcessor>> m_nodes;
    AudioBuffer m_dryBuffer;
    static constexpr uint32_t kLatencyCrossfadeSamples = 64;
    struct DryDelayState {
        DryDelayState() : lines{
            std::array<DSP::Effects::DelayLine, 2>{
                DSP::Effects::DelayLine(65536), DSP::Effects::DelayLine(65536)},
            std::array<DSP::Effects::DelayLine, 2>{
                DSP::Effects::DelayLine(65536), DSP::Effects::DelayLine(65536)}} {}
        std::array<std::array<DSP::Effects::DelayLine, 2>, 2> lines;
        uint32_t activeLatency = 0;
        uint32_t pendingLatency = 0;
        uint32_t transitionRemaining = 0;
        uint32_t activeLine = 0;
    };
    std::vector<DryDelayState> m_dryDelayStates;
    std::vector<bool> m_faultedNodes;
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 512;
    bool m_prepared = false;
    std::atomic<uint64_t> m_sanitizedSamples{0};
    std::atomic<uint64_t> m_rejectedBlocks{0};
    std::atomic<uint64_t> m_processorFaults{0};
};

} // namespace Aura::Core
