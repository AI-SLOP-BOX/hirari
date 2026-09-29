#pragma once

#include <cmath>
#include <cstdio>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host and automation adapter for the Rust stereo imager. */
class StereoImager final : public IProcessor {
public:
    explicit StereoImager(double sampleRate = 44'100.0)
        : m_state(hirari_stereo_imager_create(sampleRate)) {}
    ~StereoImager() override { hirari_stereo_imager_destroy(m_state); }

    StereoImager(const StereoImager&) = delete;
    StereoImager& operator=(const StereoImager&) = delete;

    std::string getName() const override { return "Stereo Imager"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) setWidth(value * 4.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id == 0 ? getWidth() * 0.25f : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Width" : "");
    }

    void setWidth(float width) noexcept { hirari_stereo_imager_set_width(m_state, width); }
    float getWidth() const noexcept { return hirari_stereo_imager_get_width(m_state); }
    void setSampleRate(double sampleRate) noexcept {
        hirari_stereo_imager_set_sample_rate(m_state, sampleRate);
    }

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        setSampleRate(sampleRate);
        reset();
    }

    void process(float* left, float* right, uint32_t frames) noexcept {
        if (!m_bypassed && left && right && frames != 0) {
            hirari_stereo_imager_process(m_state, left, right, frames, m_mix);
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        process(buffer.getWritePointer(0), buffer.getWritePointer(1), buffer.getNumSamples());
    }

    uint32_t getLatencySamples() const noexcept override { return 0; }
    void reset() noexcept override { hirari_stereo_imager_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
