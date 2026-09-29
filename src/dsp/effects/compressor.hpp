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
    static constexpr uint32_t kStateMagic = 0x41524350u; // "ARCP"
    static constexpr uint32_t kStateVersion = 1u;
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
        static constexpr const char* names[] = {
            "Threshold", "Ratio", "Attack", "Release", "Makeup", "Knee", "Lookahead"
        };
        const char* name = id < 7 ? names[id] : "";
        std::strncpy(outName, name, maxSize - 1);
        outName[maxSize - 1] = '\0';
    }

    std::vector<uint8_t> getState() const override {
        struct State {
            uint32_t magic, version;
            float threshold, ratio, makeup, knee;
            uint32_t lookahead;
        } state{
            kStateMagic, kStateVersion,
            getControl(1), getControl(2), getControl(5), getControl(7),
            getLatencySamples()
        };
        std::vector<uint8_t> bytes(sizeof(state));
        std::memcpy(bytes.data(), &state, sizeof(state));
        return bytes;
    }

    bool restoreStateChecked(const std::vector<uint8_t>& bytes) override {
        struct LegacyState { float threshold, ratio, makeup, knee; uint32_t lookahead; };
        struct State {
            uint32_t magic, version;
            float threshold, ratio, makeup, knee;
            uint32_t lookahead;
        };
        State state{};
        if (bytes.size() == sizeof(LegacyState)) {
            LegacyState legacy{};
            std::memcpy(&legacy, bytes.data(), sizeof(legacy));
            state = {kStateMagic, 0u, legacy.threshold, legacy.ratio, legacy.makeup,
                     legacy.knee, legacy.lookahead};
        } else if (bytes.size() == sizeof(State)) {
            std::memcpy(&state, bytes.data(), sizeof(state));
        } else {
            return false;
        }
        if (state.magic != kStateMagic || state.version > kStateVersion) return false;
        return hirari_dynamic_compressor_restore(
            m_state, state.threshold, state.ratio, state.makeup, state.knee, state.lookahead);
    }

private:
    void setControl(uint32_t control, float value) {
        hirari_dynamic_compressor_set_control(m_state, control, value);
    }
    float getControl(uint32_t control) const {
        return hirari_dynamic_compressor_get_control(m_state, control);
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
