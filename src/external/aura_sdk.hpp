#pragma once

#include <cstdint>
#include <string>
#include <vector>

/**
 * @namespace AuraSDK
 * @brief THE SOVEREIGN PLUG-IN FRAMEWORK.
 * 
 * This SDK allows developers to build high-performance instruments and effects 
 * native to the Aura Studio Pro environment. It provides direct access to the 
 * SIMD-optimized processing pipeline and the high-density Slint UI bridge.
 */
namespace AuraSDK {

    struct Version { uint32_t major, minor, patch; };
    struct ProcessData {
        float** inputs;
        float** outputs;
        uint32_t numInputs, numOutputs;
        uint32_t numSamples;
        double sampleRate;
        uint64_t playheadPos;
        bool isPlaying;
    };

    /**
     * @class IPlugin
     * @brief Base interface for all Aura-native processors.
     */
    class IPlugin {
    public:
        virtual ~IPlugin() = default;
        virtual void initialize(double sampleRate) = 0;
        virtual void process(ProcessData& data) = 0;
        virtual const char* getName() const = 0;
        virtual const char* getVendor() const = 0;
        virtual Version getVersion() const = 0;
        
        // --- PARAMETER SYSTEM ---
        struct ParameterInfo {
            uint32_t id;
            const char* name;
            float min, max, defaultValue;
            bool isAutomatable;
        };
        virtual uint32_t getNumParameters() const = 0;
        virtual void getParameterInfo(uint32_t index, ParameterInfo& info) = 0;
        virtual void setParameter(uint32_t id, float value) = 0;
        virtual float getParameter(uint32_t id) const = 0;
    };

    // --- FACTORY MACROS ---
    #define AURA_PLUGIN_EXPORT extern "C" [[maybe_unused]] AuraSDK::IPlugin* createInstance()

    // --- INDUSTRIAL UTILITIES ---
    /**
     * @class FastDSP
     * @brief A library of SIMD-ready math functions for plugin developers.
     */
    class FastDSP {
    public:
        static inline float lerp(float a, float b, float t) { return a + t * (b - a); }
        static inline float dbToLinear(float db) { return std::pow(10.0f, db / 20.0f); }
        static inline float linearToDb(float lin) { return 20.0f * std::log10(lin + 1e-9f); }
    };

} // namespace AuraSDK
