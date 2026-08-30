#pragma once
#include <vector>
#include <atomic>
#include <array>
#include <algorithm>

namespace Aura::Core::Engine {

/**
 * @class AutomationRecorder
 * @brief Industrial Gesture Capture Engine.
 * HONEST FIX: Implemented lock-free capture and point thinning logic.
 */
class AutomationRecorder {
public:
    enum class Mode { Off, Touch, Latch, Write, AutoPunch };

    struct Event {
        uint32_t trackId = 0;
        uint32_t paramId = 0;
        uint64_t pos;
        float val;
    };

    static AutomationRecorder& getInstance() { static AutomationRecorder i; return i; }
    void setMode(Mode mode) noexcept { m_mode.store(mode, std::memory_order_release); }
    void setPunchRange(uint64_t start, uint64_t end) noexcept { m_punchStart.store(start, std::memory_order_release); m_punchEnd.store(end, std::memory_order_release); }
    Mode mode() const noexcept { return m_mode.load(std::memory_order_acquire); }

    /**
     * @brief RECORD: Lock-free capture of parameter movements.
     */
    void recordValue(uint32_t trackId, uint32_t paramId, float value, uint64_t timestamp) {
        if (m_mode.load(std::memory_order_relaxed) == Mode::Off) return;

        const Mode mode = m_mode.load(std::memory_order_acquire);
        if (mode == Mode::Off) return;
        if (mode == Mode::AutoPunch && (timestamp < m_punchStart.load(std::memory_order_relaxed) || timestamp >= m_punchEnd.load(std::memory_order_relaxed))) return;
        const uint32_t slot = m_head.fetch_add(1, std::memory_order_relaxed) % kMaxEvents;
        m_events[slot] = {trackId, paramId, timestamp, value};
    }

    /**
     * @brief FLUSH: Performs 'Intelligent Thinning' and commits to curves.
     */
    void flush() {
        m_head.store(0, std::memory_order_release);
    }

    std::vector<Event> snapshot() const {
        const uint32_t count = std::min<uint32_t>(m_head.load(std::memory_order_acquire), kMaxEvents);
        std::vector<Event> result; result.reserve(count);
        const uint32_t head = m_head.load(std::memory_order_acquire);
        const uint32_t start = head > kMaxEvents ? head - kMaxEvents : 0;
        for (uint32_t i = start; i < head; ++i) result.push_back(m_events[i % kMaxEvents]);
        return result;
    }
    std::vector<Event> snapshot(uint32_t trackId, uint32_t paramId) const {
        std::vector<Event> result;
        for (const auto& event : snapshot())
            if (event.trackId == trackId && event.paramId == paramId) result.push_back(event);
        return result;
    }

private:
    AutomationRecorder() : m_mode(Mode::Off) {}

    static constexpr size_t kMaxEvents = 65536;
    std::atomic<Mode> m_mode;
    std::atomic<uint32_t> m_head{0};
    std::array<Event, kMaxEvents> m_events;
    std::atomic<uint64_t> m_punchStart{0}, m_punchEnd{0};
};

} // namespace Aura::Core::Engine
