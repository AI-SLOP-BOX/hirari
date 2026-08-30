#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <array>
#include <mutex>
#include "../midi_buffer.hpp"
#include "../utils/ring_buffer.hpp"

namespace Aura::Core::Engine {

/**
 * @struct MPENoteState
 * @brief State tracking for a single MPE note-per-channel.
 */
struct MPENoteState {
    bool active = false;
    uint8_t noteNumber;
    uint8_t channel;
    float pressure;
    float timbre;
    float bend;
};

/**
 * @class MIDIOrchestrator
 * @brief Industrial MIDI orchestration engine.
 * Handles MPE, SysEx handshakes, and Articulation ID mapping at scale.
 */
class MIDIOrchestrator {
public:
    MIDIOrchestrator() : m_mpeEnabled(false) { m_mpeNotes.fill({}); }

    void setMPEEnabled(bool enabled) noexcept {
        m_mpeEnabled.store(enabled, std::memory_order_release);
        m_resetMpe.store(true, std::memory_order_release);
    }
    bool isMPEEnabled() const noexcept {
        return m_mpeEnabled.load(std::memory_order_acquire);
    }

    void processMPE(MidiBuffer& buffer);

    // --- SYSEX ORCHESTRATION ---
    struct SysExBuffer {
        uint32_t manufacturerId;
        std::vector<uint8_t> data;
    };
    void sendSysEx(const SysExBuffer& buffer);
    void handleIncomingSysEx(const uint8_t* data, size_t size);
    bool tryPopSysEx(SysExBuffer& buffer) noexcept;

    // --- ARTICULATION ORCHESTRATION ---
    struct ArticulationMap {
        uint32_t id;
        char name[64];
        uint32_t triggerChannel;
    };
    void setArticulationMap(const std::vector<ArticulationMap>& maps);

    // --- REAL-TIME DISPATCH ---
    void processBlock(MidiBuffer& buffer);

private:
    std::atomic<bool> m_mpeEnabled;
    std::atomic<bool> m_resetMpe{false};
    std::array<MPENoteState, 16> m_mpeNotes;
    
    // Double-Buffered Articulation Maps for Lock-Free RT Access
    struct ArticulationBuffer {
        ArticulationMap maps[64];
        uint32_t count = 0;
    };
    ArticulationBuffer m_artBuffers[2];
    std::atomic<uint32_t> m_activeArtIdx{0};

    // Lock-Free SysEx Queue
    ::Aura::Core::RingBuffer<SysExBuffer, 32> m_sysExQueue;
    // RingBuffer is SPSC; serialize control-side producers (outgoing sends
    // and incoming-device dispatch) before publishing into it.
    std::mutex m_sysExProducerMutex;
};

} // namespace Aura::Core::Engine
