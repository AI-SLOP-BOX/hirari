#pragma once

#include <cstdint>
#include <string>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct Take {
    uint32_t id;
    std::string name;
    uint64_t startSample;
    uint64_t endSample;
};

/** C++ compatibility facade; take and comp-segment state lives in Rust. */
class CompingEngine {
public:
    struct CompSegment {
        uint32_t takeId;
        uint64_t start;
        uint64_t len;
        uint32_t crossfadeSamples = 256;

        bool operator<(const CompSegment& other) const { return start < other.start; }
    };

    CompingEngine() : m_state(hirari_comping_legacy_create()) {}
    ~CompingEngine() { hirari_comping_legacy_destroy(m_state); }

    CompingEngine(const CompingEngine& other)
        : m_state(hirari_comping_legacy_clone(other.m_state)) {}
    CompingEngine& operator=(const CompingEngine& other) {
        if (this == &other) return *this;
        void* replacement = hirari_comping_legacy_clone(other.m_state);
        hirari_comping_legacy_destroy(m_state);
        m_state = replacement;
        return *this;
    }

    CompingEngine(CompingEngine&& other) noexcept : m_state(other.m_state) {
        other.m_state = nullptr;
    }
    CompingEngine& operator=(CompingEngine&& other) noexcept {
        if (this == &other) return *this;
        hirari_comping_legacy_destroy(m_state);
        m_state = other.m_state;
        other.m_state = nullptr;
        return *this;
    }

    void addTake(const Take& take) {
        (void)hirari_comping_legacy_add_take(
            m_state, take.id, take.startSample, take.endSample);
    }

    uint32_t getActiveTakeAt(uint64_t sample) const {
        return hirari_comping_legacy_active_take_at(m_state, sample);
    }

    void setCompSegment(const CompSegment& segment) {
        (void)hirari_comping_legacy_set_segment(
            m_state, segment.takeId, segment.start, segment.len);
    }

    void clear() noexcept { hirari_comping_legacy_clear(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
