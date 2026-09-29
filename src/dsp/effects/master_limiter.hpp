#pragma once
#include <cstdio>
#include <vector>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

/**
 * @class MasterLimiter
 * @brief Professional Look-ahead Peak Limiter.
 * Features circular look-ahead buffer and smooth release envelope.
 * HONEST FIX: Purged fraudulent oversampling claims and unused SIMD code.
 */
class MasterLimiter : public IProcessor {
public:
    static constexpr uint32_t kMaxLookahead = 2048; 

    MasterLimiter(double sr = 44100.0) : m_state(hirari_master_limiter_create(sr)) {
        prepareToPlay(sr, 1024);
    }
    ~MasterLimiter() { hirari_master_limiter_destroy(m_state); }
    MasterLimiter(const MasterLimiter&) = delete;
    MasterLimiter& operator=(const MasterLimiter&) = delete;

    std::string getName() const override { return "HIRARI Master Limiter"; }

    uint32_t getLatencySamples() const noexcept override {
        return hirari_master_limiter_latency(m_state);
    }
    uint32_t getTailSamples() const noexcept override {
        return hirari_master_limiter_tail(m_state);
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        hirari_master_limiter_prepare(m_state, sr);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;
        hirari_master_limiter_process(m_state, l, r, numSamples, r != nullptr);
    }

    void setThreshold(float db) { hirari_master_limiter_set_control(m_state, 0, db); }
    void setCeiling(float db) { hirari_master_limiter_set_control(m_state, 1, db); }
    void setRelease(float ms) { hirari_master_limiter_set_control(m_state, 2, ms); }
    void setLookaheadMs(float ms) { hirari_master_limiter_set_control(m_state, 3, ms); }

    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_master_limiter_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_master_limiter_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Threshold", "Ceiling", "Release", "Lookahead"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32);
        const size_t written = hirari_master_limiter_save_state(
            m_state, m_bypassed ? 1u : 0u, m_mix, m_sidechainBusId,
            state.data(), state.size());
        if (written != state.size()) state.clear();
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        uint8_t bypassed = 0;
        float mix = 0.0f;
        uint32_t sidechainBusId = 0;
        if (!hirari_master_limiter_restore_state(
                m_state, state.data(), state.size(), &bypassed, &mix, &sidechainBusId)) {
            return false;
        }
        m_bypassed = bypassed != 0;
        m_mix = mix;
        m_sidechainBusId = sidechainBusId;
        return true;
    }

    void reset() noexcept override {
        hirari_master_limiter_reset(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
