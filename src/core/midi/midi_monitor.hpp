#pragma once

#include <array>
#include <cstdint>
#include <deque>
#include <vector>

namespace Aura::Core::MIDI {

struct MonitorEvent { uint64_t timestamp = 0; uint8_t status = 0, data1 = 0, data2 = 0; };
struct ChannelStats { uint64_t messages = 0, notesOn = 0, notesOff = 0, controllers = 0, pitchBends = 0; };

class MidiMonitor {
public:
    explicit MidiMonitor(size_t capacity = 4096) : m_capacity(capacity ? capacity : 1) {}
    void push(MonitorEvent event) noexcept {
        if (m_events.size() >= m_capacity) m_events.pop_front(); m_events.push_back(event);
        const uint8_t type = event.status & 0xf0, ch = event.status & 0x0f; ++m_stats[ch].messages;
        if (type == 0x90 && event.data2) ++m_stats[ch].notesOn;
        else if (type == 0x80 || type == 0x90) ++m_stats[ch].notesOff;
        else if (type == 0xb0) ++m_stats[ch].controllers;
        else if (type == 0xe0) ++m_stats[ch].pitchBends;
    }
    void clear() noexcept { m_events.clear(); m_stats.fill({}); }
    std::vector<MonitorEvent> recent(size_t count = 0) const { if (!count || count > m_events.size()) count=m_events.size(); return std::vector<MonitorEvent>(m_events.end()-static_cast<std::ptrdiff_t>(count),m_events.end()); }
    const ChannelStats& stats(uint8_t channel) const noexcept { return m_stats[channel & 0x0f]; }
    size_t size() const noexcept { return m_events.size(); }
private:
    size_t m_capacity; std::deque<MonitorEvent> m_events; std::array<ChannelStats,16> m_stats{};
};
}
