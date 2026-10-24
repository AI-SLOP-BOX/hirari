#pragma once
#include <stdint.h>
#include <vector>
#include <cmath>
#include "../audio_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

extern "C" {
void* hirari_region_stretch_create();
void hirari_region_stretch_destroy(void* handle);
bool hirari_region_stretch_prepare(void* handle, double sampleRate, uint32_t maxBlockSize);
}

// Rust owns the streaming state, scratch buffers, source preparation and seek
// decisions. This RAII adapter preserves the current Track-facing API.
class RegionTimeStretch {
public:
    RegionTimeStretch() : m_rustState(hirari_region_stretch_create()) {}
    ~RegionTimeStretch() { hirari_region_stretch_destroy(m_rustState); }
    RegionTimeStretch(const RegionTimeStretch&) = delete;
    RegionTimeStretch& operator=(const RegionTimeStretch&) = delete;

    bool prepare(double sampleRate, uint32_t maxBlockSize) {
        return m_rustState && hirari_region_stretch_prepare(m_rustState, sampleRate, maxBlockSize);
    }

    void* rustState() const noexcept { return m_rustState; }

private:
    void* m_rustState = nullptr;
};

} // namespace Hirari::Core::Engine
