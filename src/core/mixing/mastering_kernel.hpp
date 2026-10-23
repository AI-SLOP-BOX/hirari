#pragma once

#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Mixing {

/** Native API adapter for the Rust RMS, DC-block and true-peak mastering kernel. */
class MasteringKernel {
public:
    MasteringKernel() : m_state(hirari_mastering_kernel_create()) {}
    ~MasteringKernel() { hirari_mastering_kernel_destroy(m_state); }
    MasteringKernel(const MasteringKernel&) = delete;
    MasteringKernel& operator=(const MasteringKernel&) = delete;

    void setTargetLoudness(float rms) { hirari_mastering_kernel_set_target(m_state, rms); }
    void process(float* left, float* right, uint32_t samples) {
        hirari_mastering_kernel_process(m_state, left, right, samples);
    }
    float getCurrentRMSdBFS() const { return hirari_mastering_kernel_rms_dbfs(m_state); }
    float getLoudnessLUFS() const { return getCurrentRMSdBFS(); }
    float* getWritePointer(int channel) {
        return hirari_mastering_kernel_write_pointer(m_state, channel);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Mixing
