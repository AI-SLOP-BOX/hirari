#pragma once

#include <vector>
#include <memory>
#include <atomic>
#include <mutex>
#include <thread>
#include <algorithm>
#include <cmath>
#include <array>
#include <chrono>
#include <limits>
#include "../dsp/iprocessor.hpp"
#include "engine/sidechain_manager.hpp"

namespace Hirari::Core {

/**
 * @class EffectChain
 * @brief Thread-safe DSP Signal Orchestrator: Manages ordered plugin chains per track.
 *
 * Thread Safety Design (Lock-Free RT Processing):
 * - C++ owns IProcessor objects and serializes processor edits under m_mutex.
 * - Rust owns immutable realtime snapshots, traverses them without locks or
 *   allocation, and gates readers against control mutations. C++ retains shared
 *   processor ownership until Rust reports that callback readers have exited.
 */
class EffectChain {
public:
    struct Entry {
        std::shared_ptr<DSP::IProcessor> processor;
        bool bypassed = false;
        bool parallel = false;
    };

    explicit EffectChain(Engine::SidechainManager* sidechainManager = nullptr)
        : m_sidechainManager(sidechainManager ? sidechainManager
                                               : &Engine::SidechainManager::getInstance()),
          m_rustState(hirari_effect_chain_runtime_create()) {}

    ~EffectChain() {
        // Track destruction is a control-thread operation. Wait only here,
        // never in process(), so the final generation cannot leak or be freed
        // while a callback still traverses it.
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (hirari_effect_chain_runtime_audio_reader_count(m_rustState) != 0 &&
               std::chrono::steady_clock::now() < deadline) {
            std::this_thread::yield();
        }
        // A device callback that failed to quiesce must not deadlock process
        // shutdown forever. Do not reclaim retired snapshots after a timeout:
        // leaking the detached generation is safer than freeing it while an
        // audio thread may still be traversing it.
        if (hirari_effect_chain_runtime_audio_reader_count(m_rustState) != 0) {
            // Runtime snapshots contain raw processor addresses. Keep both
            // those objects and the Rust snapshot storage alive if shutdown
            // could not quiesce the callback within the bounded wait.
            auto* retained = new std::vector<Entry>();
            retained->reserve(m_pendingProcessors.size() + m_retiredProcessorOwners.size());
            for (auto& entry : m_pendingProcessors) retained->push_back(std::move(entry));
            for (auto& entry : m_retiredProcessorOwners) retained->push_back(std::move(entry));
            m_pendingProcessors.clear();
            m_retiredProcessorOwners.clear();
            m_rustState = nullptr;
            return;
        }
        reclaimRetired();
        hirari_effect_chain_runtime_destroy(m_rustState);
        m_rustState = nullptr;
    }

    /**
     * @brief Add a processor. UI/Message thread only.
     */
    void addProcessor(std::shared_ptr<DSP::IProcessor> proc, bool parallel = false) {
        if (!proc) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_pendingProcessors.push_back({proc, false, parallel});
        if (m_sampleRate > 0.0 && m_maxBlockSize > 0) {
            proc->prepareToPlay(m_sampleRate, m_maxBlockSize);
        }
        publishNewList();
    }

    /**
     * @brief Safe no-op interface mapping. Replaced try_lock with lock-free atomic pointer load.
     */
    void syncToAudioThread() noexcept {
        // No-op. The Rust-owned immutable snapshot is already published.
    }

    /**
     * @brief Process the Rust-published processor snapshot on the audio thread.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const DSP::ProcessContext& context) {
        processThroughRust(buffer, midi, context, false, 0);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext& context, uint32_t trackId) {
        processThroughRust(buffer, midi, context, true, trackId);
    }

    uint32_t getTotalLatencySamples() const {
        return hirari_effect_chain_runtime_total_latency_samples(m_rustState);
    }

    // Maximum post-input duration needed for an offline bounce.  Serial
    // processors accumulate their tails; parallel sends only extend the
    // longest branch.  This is intentionally a control-thread query so the
    // renderer can append enough silence without touching the RT list.
    uint32_t getTotalTailSamples() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return static_cast<uint32_t>(hirari_effect_chain_runtime_metric(
            m_rustState, 2, &EffectChain::readProcessorMetric));
    }

    // Control-thread only: drains processor watchdog edges without touching
    // the immutable realtime list from the audio callback.
    uint32_t takeWatchdogTrips() {
        std::lock_guard<std::mutex> lock(m_mutex);
        return static_cast<uint32_t>(hirari_effect_chain_runtime_metric(
            m_rustState, 0, &EffectChain::readProcessorMetric));
    }

    uint64_t nonFiniteSampleCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return hirari_effect_chain_runtime_metric(
            m_rustState, 1, &EffectChain::readProcessorMetric);
    }

    /**
     * @brief Set sample rate on all processors. UI/Message thread only.
     */
    void setSampleRate(double sr, uint32_t blockSize) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (!std::isfinite(sr) || sr <= 0.0 || blockSize == 0) return;
        beginAudioMutation();
        m_sampleRate = sr;
        m_maxBlockSize = blockSize;
        m_parallelBuffer.resize(2, blockSize);
        for (auto& entry : m_pendingProcessors) {
            if (entry.processor) entry.processor->prepareToPlay(sr, blockSize);
        }
        endAudioMutation();
        publishNewList();
    }

    void reset() noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        beginAudioMutation();
        for (auto& entry : m_pendingProcessors) {
            if (entry.processor) entry.processor->reset();
        }
        endAudioMutation();
    }

    /**
     * @brief Set bypass state for a processor by index. UI/Message thread only.
     */
    void setBypass(uint32_t index, bool bypassed) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (index < m_pendingProcessors.size()) {
            m_pendingProcessors[index].bypassed = bypassed;
            publishNewList();
        }
    }

    bool getBypass(uint32_t index) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return index < m_pendingProcessors.size() && m_pendingProcessors[index].bypassed;
    }

    // Audio-thread automation path. It reads the published immutable list and
    // calls the processor's noexcept parameter setter without taking the
    // control-plane mutex or changing shared_ptr ownership.
    bool setParameterRealtime(uint32_t processorIndex, uint32_t parameterId,
                              float value, uint32_t sampleOffset = 0) noexcept {
        if (!std::isfinite(value)) return false;
        ReaderGuard reader(m_rustState);
        if (!reader.active()) return false;
        auto* processor = static_cast<DSP::IProcessor*>(
            hirari_effect_chain_runtime_processor_at(m_rustState, processorIndex));
        if (!processor) return false;
        processor->setParameterAtSample(parameterId, value, sampleOffset);
        return true;
    }

    // Apply a sorted automation batch under one audio-reader guard. A single
    // guard per control point would add two atomics for every scheduled value
    // and inflate callback cost on dense curves.
    void setParameterAutomationRealtime(
        const DSP::TimedParameterEvent* events, size_t eventCount) noexcept {
        if (!events || eventCount == 0) return;
        ReaderGuard reader(m_rustState);
        if (!reader.active()) return;
        for (size_t index = 0; index < eventCount; ++index) {
            const auto& event = events[index];
            if (!std::isfinite(event.normalizedValue)) continue;
            auto* processor = static_cast<DSP::IProcessor*>(
                hirari_effect_chain_runtime_processor_at(m_rustState, event.processorIndex));
            if (processor) {
                processor->setParameterAtSample(
                    event.parameterId, event.normalizedValue, event.sampleOffset);
            }
        }
    }

    bool setParameter(uint32_t processorIndex, uint32_t parameterId, float value) {
        return setParameterNormalized(processorIndex, parameterId, value);
    }

    // Compatibility API: project/UI callers historically provide normalized
    // automation values. Keep that contract explicit instead of hiding the
    // conversion in a generic parameter setter.
    bool setParameterNormalized(uint32_t processorIndex, uint32_t parameterId, float value) {
        if (!std::isfinite(value)) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size()) return false;
        auto& processor = m_pendingProcessors[processorIndex].processor;
        if (!processor) return false;
        // The pending list shares processor instances with every published
        // generation.  Wait for readers before mutating the instance itself;
        // exchanging the vector alone does not protect plugin state.
        beginAudioMutation();
        processor->setParameter(parameterId, std::clamp(value, 0.0f, 1.0f));
        endAudioMutation();
        return true;
    }

    // Plugin adapters that expose native units (dB, Hz, milliseconds, enum
    // values) can opt into this path. The processor remains responsible for
    // validating its own descriptor/range; the host only rejects NaN/Inf.
    bool setParameterValue(uint32_t processorIndex, uint32_t parameterId, float value) {
        if (!std::isfinite(value)) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size()) return false;
        auto& processor = m_pendingProcessors[processorIndex].processor;
        if (!processor) return false;
        DSP::IProcessor::ParameterDescriptor descriptor{};
        if (processor->getParameterDescriptor(parameterId, descriptor) &&
            (!descriptor.valid() || value < descriptor.minimum || value > descriptor.maximum)) {
            return false;
        }
        beginAudioMutation();
        processor->setParameter(parameterId, value);
        endAudioMutation();
        return true;
    }

    // State access is control-thread only. Keeping it here avoids exposing the
    // immutable RT processor list to project/UI code.
    std::vector<uint8_t> getProcessorState(uint32_t processorIndex) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size() || !m_pendingProcessors[processorIndex].processor)
            return {};
        auto state = m_pendingProcessors[processorIndex].processor->getState();
        if (state.empty() && m_pendingProcessors[processorIndex].processor->getNumParameters() > 0) {
            // Keep project snapshots structurally complete for legacy
            // processors without a serializer. Parameter values are restored
            // separately, so this deterministic marker is only a non-empty
            // cache/state identity, never an audio-thread payload.
            state.resize(sizeof(float));
            const float value = m_pendingProcessors[processorIndex].processor->getParameter(0);
            std::memcpy(state.data(), &value, sizeof(float));
        }
        return state;
    }

    bool setProcessorState(uint32_t processorIndex, const std::vector<uint8_t>& state) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size() || !m_pendingProcessors[processorIndex].processor)
            return false;
        beginAudioMutation();
        bool restored = false;
        try {
            restored = m_pendingProcessors[processorIndex].processor->restoreStateChecked(state);
        } catch (...) {
            // State blobs come from project/plug-in boundaries. Never leave
            // the audio mutation gate latched if a legacy processor throws.
            restored = false;
        }
        endAudioMutation();
        return restored;
    }

    std::vector<uint8_t> getProcessorGuiState(uint32_t processorIndex) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size() || !m_pendingProcessors[processorIndex].processor) return {};
        return m_pendingProcessors[processorIndex].processor->saveGuiState();
    }

    bool setProcessorGuiState(uint32_t processorIndex, const std::vector<uint8_t>& state) {
        if (state.size() > 1024u * 1024u) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size() || !m_pendingProcessors[processorIndex].processor) return false;
        return m_pendingProcessors[processorIndex].processor->loadGuiState(state);
    }

    std::shared_ptr<DSP::IProcessor> getProcessor(uint32_t processorIndex) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (processorIndex >= m_pendingProcessors.size()) return {};
        return m_pendingProcessors[processorIndex].processor;
    }

    /**
     * @brief Reclaim retired Rust snapshots and their C++ processor owners.
     */
    void reclaimRetired() {
        std::lock_guard<std::mutex> lock(m_mutex);
        reclaimRetiredLocked();
    }

private:
    void reclaimRetiredLocked() {
        if (hirari_effect_chain_runtime_audio_reader_count(m_rustState) != 0) return;
        hirari_effect_chain_runtime_reclaim(m_rustState);
        m_retiredProcessorOwners.clear();
    }

public:
    /**
     * @brief Clear all processors. UI/Message thread only.
     */
    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_retiredProcessorOwners.insert(
            m_retiredProcessorOwners.end(), m_pendingProcessors.begin(), m_pendingProcessors.end());
        m_pendingProcessors.clear();
        publishNewList();
    }

    bool removeProcessor(uint32_t index, Entry* removed = nullptr) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (index >= m_pendingProcessors.size()) return false;
        if (removed) *removed = m_pendingProcessors[index];
        m_retiredProcessorOwners.push_back(m_pendingProcessors[index]);
        m_pendingProcessors.erase(m_pendingProcessors.begin() + index);
        publishNewList();
        return true;
    }

    // Control-thread only. Re-inserts the exact processor instance captured by
    // removeProcessor(), preserving external-plugin state and sandbox identity.
    bool insertProcessor(uint32_t index, Entry entry) {
        if (!entry.processor) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        if (index > m_pendingProcessors.size()) return false;
        m_pendingProcessors.insert(m_pendingProcessors.begin() + index, std::move(entry));
        auto& inserted = m_pendingProcessors[index].processor;
        if (inserted && m_sampleRate > 0.0 && m_maxBlockSize > 0)
            inserted->prepareToPlay(m_sampleRate, m_maxBlockSize);
        publishNewList();
        return true;
    }

    // Control-thread only. Preserve the processor instance and publish one
    // coherent replacement list to the realtime reader.
    bool moveProcessor(uint32_t from, uint32_t to) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (from >= m_pendingProcessors.size() || to >= m_pendingProcessors.size() ||
            from == to) return false;
        Entry entry = std::move(m_pendingProcessors[from]);
        m_pendingProcessors.erase(m_pendingProcessors.begin() + from);
        m_pendingProcessors.insert(m_pendingProcessors.begin() + to, std::move(entry));
        publishNewList();
        return true;
    }

private:
    static uint64_t readProcessorMetric(void* handle, uint32_t metric) noexcept {
        auto* processor = static_cast<DSP::IProcessor*>(handle);
        if (!processor) return 0;
        switch (metric) {
            case 0: return processor->takeWatchdogTrip() ? 1u : 0u;
            case 1: return processor->nonFiniteSampleCount();
            case 2: return processor->getTailSamples();
            default: return 0;
        }
    }

    struct ProcessCallbackContext {
        EffectChain* owner;
        Core::AudioBuffer* buffer;
        Core::MidiBuffer* midi;
        const DSP::ProcessContext* context;
        uint32_t trackId;
        bool withSidechain;
    };

    static bool processRealtimeNode(void* opaque, uint32_t index, void* processorHandle,
                                    bool parallel, float* mix) {
        auto* call = static_cast<ProcessCallbackContext*>(opaque);
        auto* processor = static_cast<DSP::IProcessor*>(processorHandle);
        if (!call || !mix || !processor) return false;
        Core::AudioBuffer& target = parallel ? call->owner->m_parallelBuffer : *call->buffer;
        if (call->withSidechain)
            return processRealtimeNodeWithSidechain(*call, *processor, target, index, mix);
        try {
            processor->process(target, *call->midi, *call->context);
        } catch (...) {
            return false;
        }
        *mix = processor->getMix();
        return true;
    }

    static bool processRealtimeNodeWithSidechain(
        ProcessCallbackContext& call, DSP::IProcessor& processor, Core::AudioBuffer& target,
        uint32_t index, float* mix) {
        DSP::ProcessContext pluginContext = *call.context;
        Engine::SidechainManager::LinkSnapshot sidechainSnapshot{};
        std::array<float, Engine::SidechainManager::kMaxBlockSize> sidechainLeft{};
        std::array<float, Engine::SidechainManager::kMaxBlockSize> sidechainRight{};
        const bool hasSidechain = call.context->blockSize <=
                Engine::SidechainManager::kMaxBlockSize &&
            call.owner->m_sidechainManager->copySidechainBlock(
                call.trackId, index, sidechainLeft.data(), sidechainRight.data(),
                static_cast<uint32_t>(call.context->blockSize), sidechainSnapshot);
        Core::AudioBuffer sidechainView;
        if (hasSidechain && call.context->blockSize > 0 &&
            sidechainSnapshot.frames >= call.context->blockSize &&
            static_cast<uint32_t>(call.context->sampleRate) == sidechainSnapshot.sampleRate &&
            (call.context->audioConfigGeneration == 0 ||
             sidechainSnapshot.sourceGeneration == call.context->audioConfigGeneration)) {
            float* sidechainChannels[2] = {sidechainLeft.data(), sidechainRight.data()};
            sidechainView.wrapChannels(sidechainChannels, 2,
                static_cast<uint32_t>(call.context->blockSize));
            pluginContext.sidechainBuffer = &sidechainView;
        } else {
            pluginContext.sidechainBuffer = nullptr;
        }
        try {
            processor.process(target, *call.midi, pluginContext);
        } catch (...) {
            return false;
        }
        *mix = processor.getMix();
        return true;
    }

    void processThroughRust(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                            const DSP::ProcessContext& context,
                            bool withSidechain, uint32_t trackId) {
        ReaderGuard reader(m_rustState);
        if (!reader.active()) {
            buffer.clear(buffer.getNumSamples());
            return;
        }
        ProcessCallbackContext callback{this, &buffer, &midi,
                                        &context, trackId, withSidechain};
        hirari_effect_chain_runtime_process_block(
            m_rustState, &callback,
            buffer.getArrayOfWritePointers(), buffer.getNumChannels(),
            m_parallelBuffer.getArrayOfWritePointers(), m_parallelBuffer.getNumChannels(),
            m_parallelBuffer.getNumSamples(), buffer.getNumSamples(),
            &EffectChain::processRealtimeNode);
    }

    struct ReaderGuard {
        explicit ReaderGuard(void* state)
            : m_state(state), m_active(hirari_effect_chain_runtime_enter_audio(state)) {}
        ~ReaderGuard() {
            if (m_active) hirari_effect_chain_runtime_leave_audio(m_state);
        }
        bool active() const noexcept { return m_active; }
        void* m_state;
        bool m_active;
    };

    // Control-thread mutations of a processor instance must not overlap an
    // audio callback. Rust snapshots hold raw handles; C++ retains shared
    // owners across generations until every callback reader has left.
    void beginAudioMutation() const noexcept {
        hirari_effect_chain_runtime_begin_mutation(m_rustState);
    }

    void endAudioMutation() const noexcept {
        hirari_effect_chain_runtime_end_mutation(m_rustState);
    }

    // Session-owned when injected; the process-wide singleton remains only as
    // a source-compatible fallback for legacy callers.
    Engine::SidechainManager* m_sidechainManager = nullptr;
    double m_sampleRate = 0.0;
    uint32_t m_maxBlockSize = 0;
    AudioBuffer m_parallelBuffer;
    mutable std::mutex m_mutex;
    std::vector<Entry> m_pendingProcessors;  // Owned by UI thread (under mutex)
    std::vector<Entry> m_retiredProcessorOwners; // Keep removed processors alive for old RT snapshots
    void* m_rustState = nullptr;
    // Reader admission and mutation exclusion live with the Rust runtime
    // snapshots they protect.

    // Publish immutable raw-processor metadata to Rust. C++ retains shared
    // ownership separately so no shared_ptr operation occurs on the callback.
    void publishNewList() {
        std::vector<HirariEffectChainNodeView> nodes;
        nodes.reserve(m_pendingProcessors.size());
        for (const auto& entry : m_pendingProcessors) {
            nodes.push_back({entry.processor.get(),
                             static_cast<uint8_t>(entry.bypassed),
                             static_cast<uint8_t>(entry.parallel),
                             entry.processor ? entry.processor->getLatencySamples() : 0});
        }
        (void)hirari_effect_chain_runtime_publish(m_rustState, nodes.data(), nodes.size());
        reclaimRetiredLocked();
    }
};

} // namespace Hirari::Core
