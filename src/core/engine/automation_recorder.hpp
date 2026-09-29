#pragma once

#include "../rust_ffi.hpp"

#include <algorithm>
#include <cstdint>
#include <vector>

namespace Hirari::Core::Engine {

/** Thin native API over the Rust lock-free automation recorder. */
class AutomationRecorder {
public:
    enum class Mode { Off, Touch, Latch, Write, AutoPunch };

    struct Event {
        uint32_t trackId = 0;
        uint32_t paramId = 0;
        uint64_t pos = 0;
        float val = 0.0f;
    };

    static AutomationRecorder& getInstance() {
        static AutomationRecorder instance;
        return instance;
    }

    AutomationRecorder(const AutomationRecorder&) = delete;
    AutomationRecorder& operator=(const AutomationRecorder&) = delete;

    void setMode(Mode mode) noexcept {
        hirari_automation_recorder_set_mode(state_, static_cast<uint32_t>(mode));
    }
    void setPunchRange(uint64_t start, uint64_t end) noexcept {
        hirari_automation_recorder_set_punch_range(state_, start, end);
    }
    Mode mode() const noexcept {
        return static_cast<Mode>(hirari_automation_recorder_get_mode(state_));
    }
    void recordValue(uint32_t trackId, uint32_t paramId, float value,
                     uint64_t timestamp) noexcept {
        hirari_automation_recorder_record_value(state_, trackId, paramId, value, timestamp);
    }
    void flush() noexcept { hirari_automation_recorder_flush(state_); }
    const void* stateForTrackCapture() const noexcept { return state_; }

    std::vector<Event> snapshot() const {
        size_t capacity = hirari_automation_recorder_snapshot(state_, nullptr, 0);
        for (;;) {
            std::vector<Event> events(capacity);
            static_assert(sizeof(Event) == sizeof(HirariAutomationEvent));
            static_assert(offsetof(Event, pos) == offsetof(HirariAutomationEvent, pos));
            const size_t written = hirari_automation_recorder_snapshot(
                state_, reinterpret_cast<HirariAutomationEvent*>(events.data()), capacity);
            if (written <= capacity) {
                events.resize(written);
                return events;
            }
            capacity = written;
        }
    }

    std::vector<Event> snapshot(uint32_t trackId, uint32_t paramId) const {
        auto events = snapshot();
        events.erase(std::remove_if(events.begin(), events.end(),
            [trackId, paramId](const Event& event) {
                return event.trackId != trackId || event.paramId != paramId;
            }), events.end());
        return events;
    }

private:
    AutomationRecorder() : state_(hirari_automation_recorder_create()) {}
    ~AutomationRecorder() { hirari_automation_recorder_destroy(state_); }

    void* state_ = nullptr;
};

} // namespace Hirari::Core::Engine
