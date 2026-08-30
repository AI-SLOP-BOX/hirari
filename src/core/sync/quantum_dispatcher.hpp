#pragma once
#include <cstdint>
#include <vector>
#include <queue>
#include <functional>
#include <atomic>

namespace Aura::Core::Sync {

/**
 * @struct QuantumEvent
 * @brief Time-stamped command with sub-sample precision.
 */
struct QuantumEvent {
    uint64_t sampleOffset; 
    float subSample;       
    std::function<void(float)> action; // Action takes sub-sample offset as argument

    // Min-Heap comparator (Smallest sampleOffset at the top)
    bool operator>(const QuantumEvent& other) const {
        if (sampleOffset != other.sampleOffset) return sampleOffset > other.sampleOffset;
        return subSample > other.subSample;
    }
};

/**
 * @class QuantumDispatcher
 * @brief High-precision event orchestration kernel using Priority-Queue sovereignty.
 */
class QuantumDispatcher {
public:
    static QuantumDispatcher& i() { static QuantumDispatcher d; return d; }

    /**
     * @brief Schedules an event with O(log N) complexity.
     */
    void schedule(uint64_t base, float sub, std::function<void(float)> act) {
        m_queue.push({ base, sub, std::move(act) });
    }

    /**
     * @brief Processes events for the current block window.
     * INDUSTRIAL: O(M log N) where M is the number of events in this block.
     */
    void process(uint64_t start, uint32_t sz) {
        uint64_t end = start + sz;

        // --- PHASE 45: QUANTUM-DETERMINISTIC POPPING ---
        while (!m_queue.empty() && m_queue.top().sampleOffset < end) {
            const auto& ev = m_queue.top();
            
            // Execute with sub-sample awareness
            ev.action(ev.subSample);
            
            m_queue.pop();
        }
    }

    bool empty() const { return m_queue.empty(); }

private:
    QuantumDispatcher() = default;
    
    // Using std::priority_queue with a custom comparator for O(log N) insertion
    std::priority_queue<QuantumEvent, std::vector<QuantumEvent>, std::greater<QuantumEvent>> m_queue;
};

} // namespace Aura::Core::Sync
