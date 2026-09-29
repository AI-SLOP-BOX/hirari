#pragma once

#include <array>
#include <cmath>
#include "../../core/audio_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** C++ AudioBuffer adapter for the Rust stem-mask kernel. */
class StemSplitter {
public:
    struct Stems {
        Core::AudioBuffer drums;
        Core::AudioBuffer bass;
        Core::AudioBuffer vocals;
        Core::AudioBuffer other;
    };

    Stems split(const Core::AudioBuffer& input, double sampleRate) {
        Stems result;
        const uint32_t channels = input.getNumChannels();
        const uint32_t samples = input.getNumSamples();
        if (channels == 0 || channels > Core::AudioBuffer::kMaxFastPathChannels ||
            samples == 0 || !std::isfinite(sampleRate) || sampleRate <= 0.0) {
            return result;
        }

        std::array<const float*, Core::AudioBuffer::kMaxFastPathChannels> sources{};
        for (uint32_t channel = 0; channel < channels; ++channel) {
            sources[channel] = input.getReadPointer(channel);
            if (sources[channel] == nullptr) return Stems{};
        }

        result.drums.resize(channels, samples);
        result.bass.resize(channels, samples);
        result.vocals.resize(channels, samples);
        result.other.resize(channels, samples);

        std::array<float*, Core::AudioBuffer::kMaxFastPathChannels> drums{};
        std::array<float*, Core::AudioBuffer::kMaxFastPathChannels> bass{};
        std::array<float*, Core::AudioBuffer::kMaxFastPathChannels> vocals{};
        std::array<float*, Core::AudioBuffer::kMaxFastPathChannels> other{};
        for (uint32_t channel = 0; channel < channels; ++channel) {
            drums[channel] = result.drums.getWritePointer(channel);
            bass[channel] = result.bass.getWritePointer(channel);
            vocals[channel] = result.vocals.getWritePointer(channel);
            other[channel] = result.other.getWritePointer(channel);
            if (!drums[channel] || !bass[channel] || !vocals[channel] || !other[channel])
                return Stems{};
        }

        if (!hirari_stem_split_process(sources.data(), drums.data(), bass.data(),
                                       vocals.data(), other.data(), channels,
                                       samples, sampleRate)) {
            return Stems{};
        }
        return result;
    }
};

} // namespace Hirari::DSP::Analysis
