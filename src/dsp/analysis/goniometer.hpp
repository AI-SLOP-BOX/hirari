#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** Public stereo-scope API backed by Rust correlation and history processing. */
class Goniometer {
public:
    static constexpr size_t kHistorySize = 1024;
    struct Data {
        float correlation = 1.0f;
        float balance = 0.0f;
        std::array<float, kHistorySize> xyHistoryL{};
        std::array<float, kHistorySize> xyHistoryR{};
    };

    Goniometer() : m_state(hirari_goniometer_create()) {}
    ~Goniometer() { hirari_goniometer_destroy(m_state); }

    Goniometer(const Goniometer&) = delete;
    Goniometer& operator=(const Goniometer&) = delete;

    void process(const float* left, const float* right, uint32_t samples) {
        hirari_goniometer_process(m_state, left, right, samples);
    }

    Data getLatest() const {
        Data data{};
        hirari_goniometer_snapshot(m_state, data.xyHistoryL.data(), data.xyHistoryR.data(),
                                   kHistorySize, &data.correlation, &data.balance);
        return data;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
