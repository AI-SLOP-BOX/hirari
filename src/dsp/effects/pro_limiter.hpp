#pragma once

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Host-facing adapter for the Rust-owned plugin limiter. */
class ProLimiter final : public IProcessor {
public:
    static constexpr uint32_t kLookahead = 480;

    explicit ProLimiter(double sampleRate = 44'100.0)
        : m_state(hirari_pro_limiter_create(sampleRate)) {}
    ~ProLimiter() override { hirari_pro_limiter_destroy(m_state); }

    ProLimiter(const ProLimiter&) = delete;
    ProLimiter& operator=(const ProLimiter&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_pro_limiter_prepare(m_state, sampleRate);
    }
    uint32_t getLatencySamples() const noexcept override { return kLookahead; }
    uint32_t getTailSamples() const noexcept override { return hirari_pro_limiter_tail(m_state); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        hirari_pro_limiter_process(m_state, buffer.getWritePointer(0),
                                   buffer.getWritePointer(1), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_pro_limiter_reset(m_state); }

    void setThreshold(float db) noexcept { hirari_pro_limiter_set_threshold_db(m_state, db); }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_pro_limiter_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_pro_limiter_get_parameter(m_state, id);
    }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Threshold" : "";
        std::strncpy(outName, name, maxSize - 1);
        outName[maxSize - 1] = '\0';
    }

    std::vector<uint8_t> getState() const override {
        const float threshold = hirari_pro_limiter_threshold_linear(m_state);
        std::vector<uint8_t> state(sizeof(threshold));
        std::memcpy(state.data(), &threshold, sizeof(threshold));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float)) return false;
        float threshold = 0.0f;
        std::memcpy(&threshold, state.data(), sizeof(threshold));
        return hirari_pro_limiter_restore_threshold(m_state, threshold);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        return setState(state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
