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
#include "../dsp/iprocessor.hpp"
#include "engine/sidechain_manager.hpp"

namespace Aura::Core {

/**
 * @class EffectChain
 * @brief Thread-safe DSP Signal Orchestrator: Manages ordered plugin chains per track.
 *
 * Thread Safety Design (Lock-Free RT Processing):
 * - UI/Control mutations (addProcessor, setBypass, setSampleRate, clear) are protected by m_mutex.
 *   These allocate a new immutable snapshot of the ProcessorList and swap it atomically using
 *   m_activeProcessors pointer.
 * - The real-time audio thread only loads this atomic pointer via m_activeProcessors.load().
 *   This avoids mutex lock contentions, try_lock failures, and std::vector allocation.
 * - Retired lists are registered in m_retiredLists and safely freed only on the control thread (under lock),
 *   guaranteeing no std::shared_ptr count drops or heavy object deallocations happen in the audio thread.
 */
class EffectChain {
public:
    struct Entry {
        std::shared_ptr<DSP::IProcessor> processor;
        bool bypassed = false;
        bool parallel = false;
    };

    using ProcessorList = std::vector<Entry>;

    explicit EffectChain(Engine::SidechainManager* sidechainManager = nullptr)
        : m_sidechainManager(sidechainManager ? sidechainManager
                                               : &Engine::SidechainManager::getInstance()) {
        auto list = std::make_unique<ProcessorList>();
        m_activeProcessors.store(list.release(), std::memory_order_release);
    }

    ~EffectChain() {
        if (const auto* list = m_activeProcessors.exchange(nullptr, std::memory_order_relaxed)) {
            m_retiredLists.push_back(list);
        }
        // Track destruction is a control-thread operation. Wait only here,
        // never in process(), so the final generation cannot leak or be freed
        // while a callback still traverses it.
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (m_audioReaders.load(std::memory_order_acquire) != 0 &&
               std::chrono::steady_clock::now() < deadline) {
            std::this_thread::yield();
        }
        // A device callback that failed to quiesce must not deadlock process
        // shutdown forever. Do not reclaim retired lists after a timeout:
        // leaking the detached generation is safer than freeing it while an
        // audio thread may still be traversing it.
        if (m_audioReaders.load(std::memory_order_acquire) != 0) return;
        reclaimRetired();
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
        // No-op. The audio thread reads the published pointer directly at atomic speed.
    }

    /**
     * @brief Process the active processor chain on the audio thread.
     * Reads m_activeProcessors (immutable after publishNewList on control thread).
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const DSP::ProcessContext& context) {
        ReaderGuard reader(m_audioReaders, m_audioMutation);
        if (!reader.active()) {
            buffer.clear(buffer.getNumSamples());
            return;
        }
        const auto* list = m_activeProcessors.load(std::memory_order_acquire);
        if (!list) return;

        for (const auto& entry : *list) {
            if (entry.bypassed || !entry.processor) continue;
            const bool useParallel = entry.parallel && buffer.getNumChannels() <= 2 &&
                buffer.getNumSamples() <= m_parallelBuffer.getNumSamples();
            if (useParallel) {
                for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                    std::memcpy(m_parallelBuffer.getWritePointer(c), buffer.getReadPointer(c),
                                static_cast<size_t>(buffer.getNumSamples()) * sizeof(float));
                }
            }
            try {
                entry.processor->process(useParallel ? m_parallelBuffer : buffer, midi, context);
            } catch (...) {
                buffer.clear(buffer.getNumSamples());
                continue;
            }
            if (useParallel) {
                const float mix = std::clamp(entry.processor->getMix(), 0.0f, 1.0f);
                for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                    float* dst = buffer.getWritePointer(c);
                    const float* wet = m_parallelBuffer.getReadPointer(c);
                    for (uint32_t s = 0; s < buffer.getNumSamples(); ++s)
                        dst[s] = dst[s] * (1.0f - mix) + wet[s] * mix;
                }
            }
            (void)buffer.sanitizeNonFinite();
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext& context, uint32_t trackId) {
        ReaderGuard reader(m_audioReaders, m_audioMutation);
        if (!reader.active()) {
            buffer.clear(buffer.getNumSamples());
            return;
        }
        const auto* list = m_activeProcessors.load(std::memory_order_acquire);
        if (!list) return;

        for (uint32_t index = 0; index < list->size(); ++index) {
            const auto& entry = (*list)[index];
            if (entry.bypassed || !entry.processor) continue;
            DSP::ProcessContext pluginContext = context;
            Engine::SidechainManager::LinkSnapshot sidechainSnapshot{};
            std::array<float, Engine::SidechainManager::kMaxBlockSize> sidechainLeft{};
            std::array<float, Engine::SidechainManager::kMaxBlockSize> sidechainRight{};
            const bool hasSidechain = context.blockSize <= Engine::SidechainManager::kMaxBlockSize &&
                m_sidechainManager->copySidechainBlock(trackId, index,
                    sidechainLeft.data(), sidechainRight.data(),
                    static_cast<uint32_t>(context.blockSize), sidechainSnapshot);
            float* sidechainChannels[2] = {
                hasSidechain ? sidechainLeft.data() : nullptr,
                hasSidechain ? sidechainRight.data() : nullptr
            };
            Core::AudioBuffer sidechainView;
            if (sidechainChannels[0] && sidechainChannels[1] && context.blockSize > 0 &&
                sidechainSnapshot.frames >= context.blockSize &&
                static_cast<uint32_t>(context.sampleRate) == sidechainSnapshot.sampleRate &&
                (context.audioConfigGeneration == 0 ||
                 sidechainSnapshot.sourceGeneration == context.audioConfigGeneration)) {
                sidechainView.wrapChannels(sidechainChannels, 2, context.blockSize);
                pluginContext.sidechainBuffer = &sidechainView;
            } else {
                pluginContext.sidechainBuffer = nullptr;
            }
            const bool useParallel = entry.parallel && buffer.getNumChannels() <= 2 &&
                buffer.getNumSamples() <= m_parallelBuffer.getNumSamples();
            if (useParallel) {
                for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                    std::memcpy(m_parallelBuffer.getWritePointer(c), buffer.getReadPointer(c),
                                static_cast<size_t>(buffer.getNumSamples()) * sizeof(float));
                }
            }
            try {
                entry.processor->process(useParallel ? m_parallelBuffer : buffer, midi, pluginContext);
            } catch (...) {
                buffer.clear(buffer.getNumSamples());
                continue;
            }
            if (useParallel) {
                const float mix = std::clamp(entry.processor->getMix(), 0.0f, 1.0f);
                for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                    float* dst = buffer.getWritePointer(c);
                    const float* wet = m_parallelBuffer.getReadPointer(c);
                    for (uint32_t s = 0; s < buffer.getNumSamples(); ++s)
                        dst[s] = dst[s] * (1.0f - mix) + wet[s] * mix;
                }
            }
            (void)buffer.sanitizeNonFinite();
        }
    }

    uint32_t getTotalLatencySamples() const {
        return m_totalLatency.load(std::memory_order_relaxed);
    }

    // Control-thread only: drains processor watchdog edges without touching
    // the immutable realtime list from the audio callback.
    uint32_t takeWatchdogTrips() {
        std::lock_guard<std::mutex> lock(m_mutex);
        uint32_t trips = 0;
        for (auto& entry : m_pendingProcessors) {
            if (entry.processor && entry.processor->takeWatchdogTrip()) ++trips;
        }
        return trips;
    }

    uint64_t nonFiniteSampleCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        uint64_t count = 0;
        for (const auto& entry : m_pendingProcessors) {
            if (entry.processor) count += entry.processor->nonFiniteSampleCount();
        }
        return count;
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
     * @brief Safely reclaims memory from retired processor lists if no audio threads are reading them.
     */
    void reclaimRetired() {
        if (m_audioReaders.load(std::memory_order_acquire) != 0) return;
        for (auto* list : m_retiredLists) delete list;
        m_retiredLists.clear();
    }

    /**
     * @brief Clear all processors. UI/Message thread only.
     */
    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_pendingProcessors.clear();
        publishNewList();
    }

    bool removeProcessor(uint32_t index, Entry* removed = nullptr) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (index >= m_pendingProcessors.size()) return false;
        if (removed) *removed = m_pendingProcessors[index];
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
    struct ReaderGuard {
        ReaderGuard(std::atomic<uint32_t>& readers, const std::atomic<bool>& mutation)
            : m_readers(readers), m_active(false) {
            if (mutation.load(std::memory_order_acquire)) return;
            m_readers.fetch_add(1, std::memory_order_acquire);
            if (mutation.load(std::memory_order_acquire)) {
                m_readers.fetch_sub(1, std::memory_order_release);
                return;
            }
            m_active = true;
        }
        ~ReaderGuard() {
            if (m_active) m_readers.fetch_sub(1, std::memory_order_release);
        }
        bool active() const noexcept { return m_active; }
        std::atomic<uint32_t>& m_readers;
        bool m_active;
    };

    // Control-thread mutations of a processor instance must not overlap an
    // audio callback.  The immutable list protects list ownership, but the
    // processor objects are intentionally shared between generations.
    void beginAudioMutation() const noexcept {
        m_audioMutation.store(true, std::memory_order_release);
        while (m_audioReaders.load(std::memory_order_acquire) != 0) {
            std::this_thread::yield();
        }
    }

    void endAudioMutation() const noexcept {
        m_audioMutation.store(false, std::memory_order_release);
    }

    void retireOrDelete(const ProcessorList* list) {
        if (!list) return;
        if (m_audioReaders.load(std::memory_order_acquire) == 0) delete list;
        else m_retiredLists.push_back(list);
    }

    // Session-owned when injected; the process-wide singleton remains only as
    // a source-compatible fallback for legacy callers.
    Engine::SidechainManager* m_sidechainManager = nullptr;
    double m_sampleRate = 0.0;
    uint32_t m_maxBlockSize = 0;
    AudioBuffer m_parallelBuffer;
    mutable std::mutex m_mutex;
    std::vector<Entry> m_pendingProcessors;  // Owned by UI thread (under mutex)
    std::atomic<const ProcessorList*> m_activeProcessors{nullptr}; // Loaded by audio thread
    std::vector<const ProcessorList*> m_retiredLists; // Retained to be deleted by control thread
    std::atomic<uint32_t> m_totalLatency{0};
    // Readers are counted before loading the pointer.  A publisher exchanges
    // the pointer first and only then reclaims old generations; a reader that
    // starts afterwards can therefore observe only the new generation.
    std::atomic<uint32_t> m_audioReaders{0};
    mutable std::atomic<bool> m_audioMutation{false};

    // Helper: Publishes a new immutable list to the active processors pointer,
    // and safely collects previous lists on the control thread to avoid RT deallocations.
    void publishNewList() {
        reclaimRetired();

        auto newList = std::make_unique<ProcessorList>(m_pendingProcessors);

        // Recompute total latency from new pending/active set
        uint32_t total = 0;
        for (const auto& entry : *newList) {
            if (!entry.bypassed && entry.processor) {
                total += entry.processor->getLatencySamples();
            }
        }
        m_totalLatency.store(total, std::memory_order_relaxed);

        const ProcessorList* oldList = m_activeProcessors.exchange(newList.release(), std::memory_order_release);
        if (oldList) {
            m_retiredLists.push_back(oldList);
        }
        // The exchange happens before this reclamation attempt.  Readers that
        // were already inside process() keep the old list alive; readers that
        // arrive later load the new list and cannot reference oldList.
        reclaimRetired();
    }
};

} // namespace Aura::Core
