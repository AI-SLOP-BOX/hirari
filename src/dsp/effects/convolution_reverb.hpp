#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/// Host adapter for the Rust partitioned FFT convolution reverb.
class ConvolutionReverb final : public IProcessor {
public:
    static constexpr size_t kPartitionSize = 512;
    static constexpr size_t kMaxPartitions = 32;
    static constexpr size_t kFFTSize = kPartitionSize * 2;
    enum class Model : uint32_t { WarmPlate, ConcreteRoom };

    ConvolutionReverb() : m_state(hirari_convolution_reverb_create(44'100.0)) {}
    ~ConvolutionReverb() override { hirari_convolution_reverb_destroy(m_state); }
    ConvolutionReverb(const ConvolutionReverb&) = delete;
    ConvolutionReverb& operator=(const ConvolutionReverb&) = delete;

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_convolution_reverb_prepare(m_state, sampleRate);
    }
    uint32_t getTailSamples() const noexcept override {
        return hirari_convolution_reverb_tail(m_state);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.isEmpty() || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (left && right) {
            hirari_convolution_reverb_process(
                m_state, left, right, buffer.getNumSamples(), m_mix);
        }
    }

    void setIR(Model model) noexcept {
        hirari_convolution_reverb_set_ir(m_state, static_cast<uint32_t>(model));
    }
    bool loadImpulseResponse(const std::vector<float>& impulse) noexcept {
        return hirari_convolution_reverb_load_ir(m_state, impulse.data(), impulse.size());
    }
    void reset() noexcept override { hirari_convolution_reverb_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
