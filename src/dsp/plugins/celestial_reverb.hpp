#pragma once
#include "../../core/audio_buffer.hpp"
#include "../../core/sanctuary_sdk.hpp"
#include <array>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Plugins {

class CelestialReverb : public Core::Sanctuary::ISanctuaryPlugin {
public:
    void getPluginInfo(Core::Sanctuary::PluginInfo& info) override { 
        info = {"Celestial Reverb", "Aura SAW", 100, 0xCE11}; 
    }
    
    void prepareToPlay(double, uint32_t) override {
        reset();
    }
    
    void process(Core::AudioBuffer& b) override {
        const uint32_t sz = b.getNumSamples();
        if (sz == 0) return;
        const uint32_t channels = b.getNumChannels();
        
        float* l = b.getWritePointer(0);
        float* r = channels > 1 ? b.getWritePointer(1) : nullptr;
        if (!l) return;

        const float wet = m_wet;
        const float dry = 1.0f - wet;
        const float feedback = 0.72f;

        for (uint32_t i = 0; i < sz; ++i) {
            // Left Channel DSP
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float delayOutL = m_delayBufferL[m_ptrL];
            m_delayBufferL[m_ptrL] = inL + delayOutL * feedback;
            m_ptrL = (m_ptrL + 1) % kDelaySizeL;
            l[i] = inL * dry + delayOutL * wet;

            // Right Channel DSP (Stereo widening via decoupled delay times)
            if (r) {
                const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
                const float delayOutR = m_delayBufferR[m_ptrR];
                m_delayBufferR[m_ptrR] = inR + delayOutR * feedback;
                m_ptrR = (m_ptrR + 1) % kDelaySizeR;
                r[i] = inR * dry + delayOutR * wet;
            }
        }
    }
    
    void release() override { delete this; }
    
    uint32_t getParamCount() override { return 1; }
    
    void getParamMetadata(uint32_t index, Core::Sanctuary::ParamMetadata& meta) override {
        if (index == 0) {
            meta.name = "Wet";
            meta.scaling = Core::Sanctuary::ParamScaling::Linear;
            meta.unit = Core::Sanctuary::ParamUnit::Percentage;
            meta.minValue = 0.0f;
            meta.maxValue = 1.0f;
            meta.defaultValue = 0.5f;
        }
    }
    
    float getParamValue(uint32_t index) override { 
        return index == 0 ? m_wet : 0.0f; 
    }
    
    void setParamValue(uint32_t index, float value) override {
        if (index == 0 && std::isfinite(value)) {
            m_wet = std::clamp(value, 0.0f, 1.0f);
        }
    }

private:
    void reset() {
        std::fill(std::begin(m_delayBufferL), std::end(m_delayBufferL), 0.0f);
        std::fill(std::begin(m_delayBufferR), std::end(m_delayBufferR), 0.0f);
        m_ptrL = 0;
        m_ptrR = 0;
    }

    static constexpr size_t kDelaySizeL = 1601; // Prime lengths to avoid resonant flutter
    static constexpr size_t kDelaySizeR = 1997;
    float m_delayBufferL[kDelaySizeL] = {};
    float m_delayBufferR[kDelaySizeR] = {};
    size_t m_ptrL = 0;
    size_t m_ptrR = 0;
    float m_wet = 0.5f;
};

} // namespace Aura::DSP::Plugins
