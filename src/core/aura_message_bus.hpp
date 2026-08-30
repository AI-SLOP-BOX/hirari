#pragma once
#include <variant>
#include <string>
#include <cstdint>
#include "utils/ring_buffer.hpp"
#include "bridge_types.hpp"

namespace Aura::Core {

/**
 * @brief AuraMessageBus: Unified sovereign communication layer for Aura DAW.
 * INDUSTRIAL: Combining commands, events, and telemetry updates into a single lock-free bus.
 * This simplifies the engine architecture by providing a one-stop-shop for inter-thread messaging.
 */
class AuraMessageBus {
public:
    /**
     * @struct Command
     * @brief High-priority engine control messages.
     */
    struct Command { 
        CommandType type; 
        uint32_t targetId; 
        float value; 
        uint64_t timestamp; 
    };

    /**
     * @struct Event
     * @brief Engine-to-UI notification messages.
     */
    struct Event { 
        uint32_t eventId; 
        uint32_t trackId; 
        float value; 
    };

    /**
     * @struct Telemetry
     * @brief Periodic performance reports.
     */
    struct Telemetry { 
        float cpuLoad; 
        uint32_t activeVoices; 
        uint32_t drawCalls;
    };

    /**
     * @struct DSPPerformance
     * @brief Per-component execution timing.
     */
    struct DSPPerformance {
        uint32_t componentId;
        float executionTimeMs;
    };
    
    /**
     * @brief The unified message variant.
     */
    using Message = std::variant<Command, Event, Telemetry, DSPPerformance>;

    // RT-Safety static checks to ensure zero-allocation message passing
    static_assert(std::is_trivially_copyable_v<Command>, "Command must be trivially copyable for RT-safety");
    static_assert(std::is_trivially_copyable_v<Event>, "Event must be trivially copyable for RT-safety");
    static_assert(std::is_trivially_copyable_v<Telemetry>, "Telemetry must be trivially copyable for RT-safety");
    static_assert(std::is_trivially_copyable_v<DSPPerformance>, "DSPPerformance must be trivially copyable for RT-safety");
    static_assert(std::is_trivially_copyable_v<Message>, "Message variant must be trivially copyable for RT-safety");

    static AuraMessageBus& getInstance() {
        static AuraMessageBus instance;
        return instance;
    }

    /**
     * @brief Pushes a message into the sovereign bus.
     */
    bool push(const Message& msg) { 
        return m_queue.push(msg); 
    }

    /**
     * @brief Pops a message from the sovereign bus.
     */
    bool pop(Message& msg) { 
        return m_queue.pop(msg); 
    }

    /**
     * @brief Checks if the bus is empty.
     * @warning Do NOT check isEmpty() before pop(). This is a transient snapshot and is 
     * subject to TOCTOU race conditions. Call pop() directly and check its boolean return value.
     */
    bool isEmpty() const {
        return m_queue.isEmpty();
    }

private:
    AuraMessageBus() = default;
    
    // INDUSTRIAL: Using the optimized Power-of-Two RingBuffer.
    RingBuffer<Message, 2048> m_queue;
};

} // namespace Aura::Core
