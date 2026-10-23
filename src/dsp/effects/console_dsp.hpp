#pragma once

#include "../../core/audio_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host adapter for the Rust console compressor and saturation kernel. */
class DivineConsole {
public:
    DivineConsole() : m_state(hirari_divine_console_create()) {}
    DivineConsole(const DivineConsole& other)
        : m_state(hirari_divine_console_clone(other.m_state)) {}
    ~DivineConsole() { hirari_divine_console_destroy(m_state); }
    DivineConsole& operator=(const DivineConsole& other) {
        if (this == &other) return *this;
        void* replacement = hirari_divine_console_clone(other.m_state);
        if (replacement) {
            hirari_divine_console_destroy(m_state);
            m_state = replacement;
        }
        return *this;
    }

    void prepareToPlay(double sampleRate, uint32_t /*maxBlockSize*/) {
        hirari_divine_console_prepare(m_state, sampleRate);
    }

    void process(::Hirari::Core::AudioBuffer& buffer, uint32_t samples,
                 const float* sidechain = nullptr) {
        const uint32_t count = std::min(samples, buffer.getNumSamples());
        if (count == 0 || buffer.getNumChannels() == 0) return;
        hirari_divine_console_process(m_state, buffer.getArrayOfWritePointers(),
                                      buffer.getNumChannels(), count, sidechain);
    }

    void reset() { hirari_divine_console_reset(m_state); }
    void setDrive(float drive) { hirari_divine_console_set_drive(m_state, drive); }
    void setParams(float thresholdDb, float ratio) {
        hirari_divine_console_set_params(m_state, thresholdDb, ratio);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
