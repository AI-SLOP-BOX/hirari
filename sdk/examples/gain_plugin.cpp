#include "../../src/external/aura_sdk.hpp"
#include <algorithm>

class ExampleGain final : public AuraSDK::IPlugin {
public:
    void initialize(double) override {}

    void process(AuraSDK::ProcessData& data) override {
        const float gain = AuraSDK::FastDSP::dbToLinear(m_gainDb);
        const uint32_t channels = std::min(data.numInputs, data.numOutputs);
        for (uint32_t c = 0; c < channels; ++c) {
            if (!data.inputs[c] || !data.outputs[c]) continue;
            for (uint32_t i = 0; i < data.numSamples; ++i)
                data.outputs[c][i] = data.inputs[c][i] * gain;
        }
    }

    const char* getName() const override { return "Aura SDK Example Gain"; }
    const char* getVendor() const override { return "Aura"; }
    AuraSDK::Version getVersion() const override { return {1, 0, 0}; }
    uint32_t getNumParameters() const override { return 1; }
    void getParameterInfo(uint32_t index, AuraSDK::IPlugin::ParameterInfo& info) override {
        if (index == 0) info = {0, "Gain (dB)", -24.0f, 24.0f, 0.0f, true};
    }
    void setParameter(uint32_t id, float value) override {
        if (id == 0) m_gainDb = std::clamp(value, -24.0f, 24.0f);
    }
    float getParameter(uint32_t id) const override { return id == 0 ? m_gainDb : 0.0f; }

private:
    float m_gainDb = 0.0f;
};

AURA_PLUGIN_EXPORT { return new ExampleGain(); }
