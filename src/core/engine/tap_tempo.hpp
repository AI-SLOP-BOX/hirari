#pragma once

#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade. Tap history and tempo estimation live in Rust.
class TapTempo {
public:
    explicit TapTempo(size_t maxTaps = 8)
        : m_state(hirari_tap_tempo_create(maxTaps)) {}

    ~TapTempo() { hirari_tap_tempo_destroy(m_state); }

    TapTempo(const TapTempo&) = delete;
    TapTempo& operator=(const TapTempo&) = delete;

    void tap(uint64_t timestampMs) noexcept {
        hirari_tap_tempo_tap(m_state, timestampMs);
    }

    void clear() noexcept { hirari_tap_tempo_clear(m_state); }

    double bpm() const noexcept { return hirari_tap_tempo_bpm(m_state); }

    size_t count() const noexcept { return hirari_tap_tempo_count(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
