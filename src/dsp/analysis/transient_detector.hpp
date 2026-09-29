#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstddef>
#include <cstdint>
#include <vector>

namespace Hirari::Core::DSP::Analysis {

struct Transient {
    uint64_t sampleIndex;
    float strength;
};

class TransientDetector {
public:
    explicit TransientDetector(double sample_rate, float lookahead_ms = 2.0f)
        : m_state(hirari_transient_detector_create(sample_rate, lookahead_ms)) {}
    ~TransientDetector() { hirari_transient_detector_destroy(m_state); }

    TransientDetector(const TransientDetector&) = delete;
    TransientDetector& operator=(const TransientDetector&) = delete;

    std::vector<Transient> analyze(const float* data, size_t sample_count,
                                   float threshold = 0.15f) {
        hirari_transient_detector_analyze(m_state, data, sample_count, threshold);
        const size_t count = hirari_transient_detector_result_count(m_state);
        std::vector<Transient> results(count);
        for (size_t index = 0; index < count; ++index) {
            if (!hirari_transient_detector_get_result(
                    m_state, index, &results[index].sampleIndex, &results[index].strength)) {
                results.resize(index);
                break;
            }
        }
        return results;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::DSP::Analysis
