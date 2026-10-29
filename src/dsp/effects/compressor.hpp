#pragma once

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host facade for the Rust-owned dynamic compressor. */
class DynamicCompressor final : public IProcessor {
public:
    static constexpr uint32_t kMaxLookahead = 4096u;

    DynamicCompressor() : m_state(hirari_dynamic_compressor_create(44'100.0)) {}
    ~DynamicCompressor() override { hirari_dynamic_compressor_destroy(m_state); }

    DynamicCompressor(const DynamicCompressor&) = delete;
    DynamicCompressor& operator=(const DynamicCompressor&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_dynamic_compressor_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0 ||
            buffer.getNumSamples() > kMaxLookahead * 16u) {
            return;
        }
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2u);
        hirari_dynamic_compressor_process(
            m_state, buffer.getWritePointer(0),
            channels > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples());
    }

    uint32_t getLatencySamples() const noexcept override {
        return hirari_dynamic_compressor_latency(m_state);
    }
    uint32_t getTailSamples() const noexcept override {
        return hirari_dynamic_compressor_tail(m_state);
    }
    void reset() noexcept override { hirari_dynamic_compressor_reset(m_state); }

    void setLookahead(float ms) { setControl(0, ms); }
    void setThreshold(float db) { setControl(1, db); }
    void setRatio(float ratio) { setControl(2, ratio); }
    void setAttack(float ms) { setControl(3, ms); }
    void setRelease(float ms) { setControl(4, ms); }
    void setMakeup(float db) { setControl(5, db); }
    void setAutoGain(bool enabled) { setControl(6, enabled ? 1.0f : 0.0f); }
    void setKnee(float db) { setControl(7, db); }
    void setUseRMS(bool enabled) { setControl(8, enabled ? 1.0f : 0.0f); }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_dynamic_compressor_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_dynamic_compressor_get_parameter(m_state, id);
    }
    uint32_t getNumParameters() const noexcept override { return 7; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        std::strncpy(outName, hirari_builtin_effect_parameter_name(0, id), maxSize - 1);
        outName[maxSize - 1] = '\0';
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> bytes(28);
        bytes.resize(hirari_dynamic_compressor_save_state(m_state, bytes.data(), bytes.size()));
        return bytes;
    }

    bool restoreStateChecked(const std::vector<uint8_t>& bytes) override {
        return hirari_dynamic_compressor_restore_state(m_state, bytes.data(), bytes.size());
    }

private:
    void setControl(uint32_t control, float value) {
        hirari_dynamic_compressor_set_control(m_state, control, value);
    }
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
