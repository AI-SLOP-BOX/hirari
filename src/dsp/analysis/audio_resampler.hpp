#pragma once

#include "../../core/rust_ffi.hpp"

#include <vector>

namespace Hirari::DSP::Analysis {

class AudioResampler {
public:
    static std::vector<float> process(
        const std::vector<float>& source, double source_rate, double target_rate) {
        const std::size_t output_size = hirari_audio_resampler_output_len(
            source.size(), source_rate, target_rate);
        if (output_size == 0) return {};

        std::vector<float> output(output_size);
        if (!hirari_audio_resampler_process(
                source.data(), source.size(), source_rate, target_rate,
                output.data(), output.size())) {
            return {};
        }
        return output;
    }
};

} // namespace Hirari::DSP::Analysis
