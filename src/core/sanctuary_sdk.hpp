/* Aura DAW Ultimate - Sanctuary Plugin SDK - (c) 2026 Aura DAW Project */
#pragma once
#include "audio_buffer.hpp"
#include <string>
#include <vector>

namespace Aura::Core::Sanctuary {

enum class ParamScaling { Linear, Logarithmic, Exponential };
enum class ParamUnit { Generic, Decibels, Hertz, Milliseconds, Percentage };

struct ParamMetadata {
    std::string name;
    ParamScaling scaling = ParamScaling::Linear;
    ParamUnit unit = ParamUnit::Generic;
    float minValue = 0.0f;
    float maxValue = 1.0f;
    float defaultValue = 0.5f;
};

struct PluginInfo {
    std::string name;
    std::string vendor;
    uint32_t version;
    uint32_t uniqueID;
    uint8_t signature[64]; 
    bool isDistributed = false;
    std::string remoteHost;
};

class ISanctuaryPlugin {
public:
    virtual ~ISanctuaryPlugin() = default;
    virtual void getPluginInfo(PluginInfo& info) = 0;
    virtual void prepareToPlay(double sampleRate, uint32_t maxBlockSize) = 0;
    virtual void process(AudioBuffer& buffer) = 0;
    virtual void release() = 0;
    
    // Parameter Sovereignty
    virtual uint32_t getParamCount() = 0;
    virtual void getParamMetadata(uint32_t index, ParamMetadata& meta) = 0;
    virtual float getParamValue(uint32_t index) = 0;
    virtual void setParamValue(uint32_t index, float value) = 0;
};

// Entry point for external shared libraries
typedef ISanctuaryPlugin* (*CreateSanctuaryPluginFunc)();

} // namespace Aura::Core::Sanctuary
