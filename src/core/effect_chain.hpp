#pragma once

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <memory>
#include <vector>
#include "../dsp/iprocessor.hpp"
#include "rust_ffi.hpp"

namespace Hirari::Core {

/**
 * @class EffectChain
 * @brief Per-track plugin invocation over the Rust-owned chain list.
 *
 * Rust owns entry order, bypass/parallel state, snapshot publication, latency
 * and diagnostic aggregation, state markers, automation dispatch and reader
 * admission (hirari-core-bridge/src/effect_chain_host.rs). C++ keeps the
 * DSP::IProcessor instances because processing is virtual, and it keeps the
 * parallel scratch buffer and sidechain view that the realtime callback fills.
 */
class EffectChain {
public:
    struct Entry {
        std::shared_ptr<DSP::IProcessor> processor;
        bool bypassed = false;
        bool parallel = false;
    };

    // Bound shared by the Rust publication slots and this reader scratch pair.
    static constexpr uint32_t kMaxSidechainBlockSize = 4096;

    explicit EffectChain(void* sidechainEngine = nullptr)
        : m_sidechainEngine(sidechainEngine ? sidechainEngine
                                            : hirari_sidechain_engine_default()),
          m_state(hirari_effect_chain_create(&kOps)) {
        // Preallocated so no control-thread resize can race an audio callback
        // that is reading the parallel channel pointers.
        m_parallelBuffer.resize(2, kMaxSidechainBlockSize);
    }

    ~EffectChain() {
        // Rust latches audio readers and, if a callback fails to quiesce
        // within its bounded wait, deliberately leaks the generation instead
        // of freeing memory a realtime thread may still traverse.
        hirari_effect_chain_destroy(m_state);
        m_state = nullptr;
    }

    EffectChain(const EffectChain&) = delete;
    EffectChain& operator=(const EffectChain&) = delete;

    /**
     * @brief Add a processor. UI/Message thread only.
     */
    void addProcessor(std::shared_ptr<DSP::IProcessor> proc, bool parallel = false) {
        if (!proc) return;
        auto* owner = new std::shared_ptr<DSP::IProcessor>(std::move(proc));
        if (!hirari_effect_chain_add(m_state, owner, owner->get(), parallel)) delete owner;
    }

    /**
     * @brief Safe no-op interface mapping. Replaced try_lock with lock-free atomic pointer load.
     */
    void syncToAudioThread() noexcept {}

    /**
     * @brief Process the Rust-published processor snapshot on the audio thread.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const DSP::ProcessContext& context) {
        process(buffer, midi, context, false, 0);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext& context, uint32_t trackId) {
        process(buffer, midi, context, true, trackId);
    }

    uint32_t getTotalLatencySamples() const {
        return hirari_effect_chain_total_latency_samples(m_state);
    }

    // Maximum post-input duration needed for an offline bounce.  Serial
    // processors accumulate their tails; parallel sends only extend the
    // longest branch.  This is intentionally a control-thread query so the
    // renderer can append enough silence without touching the RT list.
    //
    // The Rust chain is sampled with a try-lock and the last value is returned
    // on contention.  These metrics are polled on a timer by the engine-health
    // overlay, so blocking here would contend directly with parameter edits on
    // the control thread; a `noexcept` signature must not be able to stall
    // behind a descheduled lock holder either.
    uint32_t getTotalTailSamples() const noexcept {
        uint64_t value = 0;
        if (!hirari_effect_chain_metric_try(m_state, 2, &value))
            return m_lastTailSamples.load(std::memory_order_relaxed);
        const auto tail = static_cast<uint32_t>(value);
        m_lastTailSamples.store(tail, std::memory_order_relaxed);
        return tail;
    }

    // Control-thread only: drains processor watchdog edges without touching
    // the immutable realtime list from the audio callback.
    uint32_t takeWatchdogTrips() {
        return static_cast<uint32_t>(hirari_effect_chain_metric(m_state, 0));
    }

    // Non-finite output is a diagnostic counter, so returning the most recent
    // sample under contention is correct and keeps this polling path free of
    // any blocking wait.  See getTotalTailSamples() for the rationale.
    uint64_t nonFiniteSampleCount() const noexcept {
        uint64_t value = 0;
        if (!hirari_effect_chain_metric_try(m_state, 1, &value))
            return m_lastNonFiniteSamples.load(std::memory_order_relaxed);
        m_lastNonFiniteSamples.store(value, std::memory_order_relaxed);
        return value;
    }

    /**
     * @brief Set sample rate on all processors. UI/Message thread only.
     */
    void setSampleRate(double sr, uint32_t blockSize) {
        (void)hirari_effect_chain_set_sample_rate(m_state, sr, blockSize);
    }

    void reset() noexcept { hirari_effect_chain_reset(m_state); }

    /**
     * @brief Set bypass state for a processor by index. UI/Message thread only.
     */
    void setBypass(uint32_t index, bool bypassed) {
        (void)hirari_effect_chain_set_bypass(m_state, index, bypassed);
    }

    bool getBypass(uint32_t index) const {
        return hirari_effect_chain_bypass(m_state, index);
    }

    void setParameterAutomationRealtime(
        const DSP::TimedParameterEvent* events, size_t eventCount) noexcept {
        static_assert(sizeof(DSP::TimedParameterEvent) == sizeof(HirariTimedParameterEvent));
        hirari_effect_chain_parameter_automation(
            m_state, reinterpret_cast<const HirariTimedParameterEvent*>(events), eventCount);
    }

    bool setParameter(uint32_t processorIndex, uint32_t parameterId, float value) {
        return hirari_effect_chain_set_parameter(m_state, processorIndex, parameterId, value);
    }

    // State access is control-thread only. The legacy marker for processors
    // without a serializer is added by the Rust host.
    std::vector<uint8_t> getProcessorState(uint32_t processorIndex) const {
        size_t length = 0;
        uint8_t* data = hirari_effect_chain_processor_state(m_state, processorIndex, &length);
        if (!data) return {};
        std::vector<uint8_t> state(data, data + length);
        hirari_effect_chain_buffer_free(data, length);
        return state;
    }

    bool setProcessorState(uint32_t processorIndex, const std::vector<uint8_t>& state) {
        return hirari_effect_chain_processor_state_restore(
            m_state, processorIndex, state.data(), state.size());
    }

    std::vector<uint8_t> getProcessorGuiState(uint32_t processorIndex) const {
        size_t length = 0;
        uint8_t* data = hirari_effect_chain_processor_gui_state(m_state, processorIndex, &length);
        if (!data) return {};
        std::vector<uint8_t> state(data, data + length);
        hirari_effect_chain_buffer_free(data, length);
        return state;
    }

    bool setProcessorGuiState(uint32_t processorIndex, const std::vector<uint8_t>& state) {
        return hirari_effect_chain_processor_gui_state_restore(
            m_state, processorIndex, state.data(), state.size());
    }

    std::shared_ptr<DSP::IProcessor> getProcessor(uint32_t processorIndex) const {
        auto* owner = static_cast<std::shared_ptr<DSP::IProcessor>*>(
            hirari_effect_chain_owner_clone(m_state, processorIndex));
        if (!owner) return {};
        auto processor = *owner;
        delete owner;
        return processor;
    }

    /**
     * @brief Reclaim retired snapshots and their C++ processor owners.
     */
    void reclaimRetired() { hirari_effect_chain_reclaim(m_state); }

    /**
     * @brief Clear all processors. UI/Message thread only.
     */
    void clear() { hirari_effect_chain_clear(m_state); }

    bool removeProcessor(uint32_t index, Entry* removed = nullptr) {
        bool bypassed = false;
        bool parallel = false;
        auto* owner = static_cast<std::shared_ptr<DSP::IProcessor>*>(
            hirari_effect_chain_remove(m_state, index, &bypassed, &parallel));
        if (!owner) return false;
        if (removed) {
            removed->processor = *owner;
            removed->bypassed = bypassed;
            removed->parallel = parallel;
        }
        delete owner;
        return true;
    }

    // Control-thread only. Re-inserts the exact processor instance captured by
    // removeProcessor(), preserving external-plugin state and sandbox identity.
    bool insertProcessor(uint32_t index, Entry entry) {
        if (!entry.processor) return false;
        auto* owner = new std::shared_ptr<DSP::IProcessor>(std::move(entry.processor));
        if (!hirari_effect_chain_insert(m_state, index, owner, owner->get(),
                                        entry.bypassed, entry.parallel)) {
            delete owner;
            return false;
        }
        return true;
    }

    // Control-thread only. Preserve the processor instance and publish one
    // coherent replacement list to the realtime reader.
    bool moveProcessor(uint32_t from, uint32_t to) {
        return hirari_effect_chain_move(m_state, from, to);
    }

private:
    struct ProcessCallbackContext {
        EffectChain* owner;
        Core::AudioBuffer* buffer;
        Core::MidiBuffer* midi;
        const DSP::ProcessContext* context;
        uint32_t trackId;
        bool withSidechain;
    };

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext& context, bool withSidechain, uint32_t trackId) {
        ProcessCallbackContext callback{this, &buffer, &midi, &context, trackId, withSidechain};
        hirari_effect_chain_process(
            m_state, &callback,
            buffer.getArrayOfWritePointers(), buffer.getNumChannels(),
            m_parallelBuffer.getArrayOfWritePointers(), m_parallelBuffer.getNumChannels(),
            m_parallelBuffer.getNumSamples(), buffer.getNumSamples(),
            &EffectChain::processRealtimeNode);
    }

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
        std::array<float, kMaxSidechainBlockSize> sidechainLeft{};
        std::array<float, kMaxSidechainBlockSize> sidechainRight{};
        uint64_t sidechainMetadata[6]{};
        uint32_t sidechainFrames = 0;
        if (call.context->blockSize <= kMaxSidechainBlockSize) {
            sidechainFrames = hirari_sidechain_engine_copy_link(
                call.owner->m_sidechainEngine, call.trackId, index, sidechainLeft.data(),
                sidechainRight.data(), static_cast<uint32_t>(call.context->blockSize),
                sidechainMetadata);
        }
        Core::AudioBuffer sidechainView;
        if (sidechainFrames != 0 && call.context->blockSize > 0 &&
            sidechainFrames >= call.context->blockSize &&
            static_cast<uint32_t>(call.context->sampleRate) ==
                static_cast<uint32_t>(sidechainMetadata[3]) &&
            (call.context->audioConfigGeneration == 0 ||
             sidechainMetadata[4] == call.context->audioConfigGeneration)) {
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

    // Per-entry adapters: the Rust host stores opaque DSP::IProcessor handles
    // and calls back through this table for control-thread work.
    static void prepareOp(void* p, double sr, uint32_t bs) {
        static_cast<DSP::IProcessor*>(p)->prepareToPlay(sr, bs);
    }
    static void resetOp(void* p) { static_cast<DSP::IProcessor*>(p)->reset(); }
    static void setParameterOp(void* p, uint32_t id, float value) {
        static_cast<DSP::IProcessor*>(p)->setParameter(id, value);
    }
    static void setParameterAtSampleOp(void* p, uint32_t id, float value, uint32_t offset) {
        static_cast<DSP::IProcessor*>(p)->setParameterAtSample(id, value, offset);
    }
    static uint32_t latencyOp(void* p) {
        return static_cast<DSP::IProcessor*>(p)->getLatencySamples();
    }
    static size_t stateSizeOp(void* p) {
        return static_cast<DSP::IProcessor*>(p)->getState().size();
    }
    static size_t stateCopyOp(void* p, uint8_t* out, size_t capacity) {
        return copyBytes(static_cast<DSP::IProcessor*>(p)->getState(), out, capacity);
    }
    static bool stateRestoreOp(void* p, const uint8_t* data, size_t length) {
        try {
            return static_cast<DSP::IProcessor*>(p)->restoreStateChecked(restoreInput(data, length));
        } catch (...) {
            return false;
        }
    }
    static size_t guiStateSizeOp(void* p) {
        return static_cast<DSP::IProcessor*>(p)->saveGuiState().size();
    }
    static size_t guiStateCopyOp(void* p, uint8_t* out, size_t capacity) {
        return copyBytes(static_cast<DSP::IProcessor*>(p)->saveGuiState(), out, capacity);
    }
    static bool guiStateRestoreOp(void* p, const uint8_t* data, size_t length) {
        try {
            return static_cast<DSP::IProcessor*>(p)->loadGuiState(restoreInput(data, length));
        } catch (...) {
            return false;
        }
    }
    static uint32_t numParametersOp(void* p) {
        return static_cast<DSP::IProcessor*>(p)->getNumParameters();
    }
    static float parameterOp(void* p, uint32_t id) {
        return static_cast<DSP::IProcessor*>(p)->getParameter(id);
    }
    static void* cloneOwnerOp(void* owner) {
        return new std::shared_ptr<DSP::IProcessor>(
            *static_cast<std::shared_ptr<DSP::IProcessor>*>(owner));
    }
    static void destroyOwnerOp(void* owner) {
        delete static_cast<std::shared_ptr<DSP::IProcessor>*>(owner);
    }
    static uint64_t readMetricOp(void* p, uint32_t metric) {
        auto* processor = static_cast<DSP::IProcessor*>(p);
        switch (metric) {
            case 0: return processor->takeWatchdogTrip() ? 1u : 0u;
            case 1: return processor->nonFiniteSampleCount();
            case 2: return processor->getTailSamples();
            default: return 0;
        }
    }

    static size_t copyBytes(const std::vector<uint8_t>& source, uint8_t* out, size_t capacity) {
        const size_t length = std::min(source.size(), capacity);
        if (length != 0) std::memcpy(out, source.data(), length);
        return length;
    }
    static std::vector<uint8_t> restoreInput(const uint8_t* data, size_t length) {
        return length != 0 ? std::vector<uint8_t>(data, data + length) : std::vector<uint8_t>();
    }

    // Session-owned when injected; Rust's process-wide default handle is only
    // the source-compatible fallback for legacy callers.
    void* m_sidechainEngine = nullptr;
    AudioBuffer m_parallelBuffer;
    // Last sampled values for the lock-free diagnostic readers above, so a
    // contended poll returns a real measurement instead of blocking.
    mutable std::atomic<uint32_t> m_lastTailSamples{0};
    mutable std::atomic<uint64_t> m_lastNonFiniteSamples{0};
    void* m_state = nullptr;

    static constexpr HirariEffectNodeOps kOps = {
        &EffectChain::prepareOp,
        &EffectChain::resetOp,
        &EffectChain::setParameterOp,
        &EffectChain::setParameterAtSampleOp,
        &EffectChain::latencyOp,
        &EffectChain::stateSizeOp,
        &EffectChain::stateCopyOp,
        &EffectChain::stateRestoreOp,
        &EffectChain::guiStateSizeOp,
        &EffectChain::guiStateCopyOp,
        &EffectChain::guiStateRestoreOp,
        &EffectChain::numParametersOp,
        &EffectChain::parameterOp,
        &EffectChain::cloneOwnerOp,
        &EffectChain::destroyOwnerOp,
        &EffectChain::readMetricOp,
    };
};

} // namespace Hirari::Core
