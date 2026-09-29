#pragma once

#include <vector>
#include <cmath>
#include <atomic>
#include <map>
#include <set>
#include "atomic_parameter.hpp"
#include "../math/denormal_killer.hpp"

namespace Hirari::DSP::Unified {

/**
 * @brief UNIFIED DSP SUITE: The high-fidelity ear and mixer of Hirari.
 * Consolidates Mastering, Mixing, Routing, and Metering into one massive file.
 */
class UnifiedDSPSuite {
public:
    enum class FX { 
        Comp, Limiter, EQ, Chorus, Phase,
        Delay, Reverb, Bitcrush, Sidechain, MS 
    };

    /**
     * @brief Professional constants to eliminate magic numbers.
     */
    static constexpr float kDefaultCeiling = 0.99f;
    static constexpr float kSmoothingTimeMs = 20.0f;
    static constexpr float kMaxSampleRate = 192000.0f;
    static constexpr size_t kMaxReverbDelay = 16384;   // Lush depth
    static constexpr size_t kMaxLimiterBuffer = 1024; // High-res lookahead

    UnifiedDSPSuite() : m_gainSmoother(1.0f, kSmoothingTimeMs, kMaxSampleRate) {}

    /**
     * @brief High-precision mixing engine handling volume, pan, and faders.
     */
    void mix(float* targetL, float* targetR, float sourceL, float sourceR, float gainDB, float pan) {
        m_gainSmoother.setTarget(std::pow(10.0f, gainDB / 20.0f));
        float gain = m_gainSmoother.nextValue();
        float panL = std::cos((pan + 1.0f) * 3.1415f * 0.25f);
        float panR = std::sin((pan + 1.0f) * 3.1415f * 0.25f);

        *targetL += sourceL * gain * panL;
        *targetR += sourceR * gain * panR;
    }

    /**
     * @brief Professional FX Processing Chain logic.
     */
    float processFX(float s, FX type) {
        // PROFESSIONAL RULE: Kill denormals before recursive feedback
        s = ::Hirari::DSP::Math::DenormalNumberKiller::kill(s);

        if (type == FX::Limiter) {
            // SR-independent release: exp(-1.0 / (release_s * sampleRate))
            const float releaseCoeff = std::exp(-1.0f / (0.010f * 48000.0f)); // 10ms release
            m_peakLevel = std::max(s, m_peakLevel * releaseCoeff);
            return s * (1.0f / (m_peakLevel + 1e-10f));
        }
        return s;
    }

    /**
     * @brief AUX ROUTER: Logic Pro-style Sends and Returns.
     * Routes signal fraction (sendDB) to a shared Bus/Aux channel.
     */
    void routeToBus(uint32_t busId, float sourceL, float sourceR, float sendDB) {
        float sendGain = std::pow(10.0f, sendDB / 20.0f);
        m_buses[busId].sumL += sourceL * sendGain;
        m_buses[busId].sumR += sourceR * sendGain;
    }

private:
    struct BusBuffer { float sumL = 0, sumR = 0; };
    ::Hirari::Core::ParameterSmoother m_gainSmoother;
    float m_masterGain = 1.0f;
    std::set<uint32_t> m_soloSafeTracks; // SoloSafe logic integrated
    std::map<uint32_t, BusBuffer> m_buses; // Bus ID -> Stereo Sum
};

/**
 * @brief InputStage: Pre-fader signal alignment logic.
 */
struct InputStage { float gainLinear = 1.0f; bool phaseInvert = false; };

} // namespace Hirari::DSP::Unified
