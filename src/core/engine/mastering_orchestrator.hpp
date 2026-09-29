#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <array>
#include <mutex>
#include "../audio_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * @struct LoudnessMetrics
 * @brief EBU R128 compliant loudness metrics (LUFS).
 */
struct LoudnessMetrics {
    float integrated;    // Program Loudness
    float shortTerm;     // 3s sliding window
    float momentary;     // 400ms sliding window
    float range;         // LRA
    float truePeak;      // dBTP
};

/**
 * @class MasteringOrchestrator
 * @brief Global mastering and loudness management engine.
 * Orchestrates EBU R128 analysis, spectral target matching, and DDP export.
 */
class MasteringOrchestrator {
public:
    MasteringOrchestrator() 
        : m_hasTargetProfile(false), m_cachedGains{1,1,1,1,1,1,1,1},
          m_rustProcessor(hirari_mastering_processor_create()) {
        m_metrics = {-24.0f, -24.0f, -24.0f, 0.0f, -1.0f};
    }
    ~MasteringOrchestrator() { hirari_mastering_processor_free(m_rustProcessor); }

    void process(AudioBuffer& buffer);
    LoudnessMetrics getMetrics() const { return m_metrics; }

    // --- SPECTRAL MATCHING ---
    struct SpectralProfile {
        std::vector<float> bins;
    };
    void analyzeSpectralProfile(const AudioBuffer& buffer);
    void applyTargetProfile(const SpectralProfile& target);

    // --- DDP EXPORT ---
    struct DDPConfig {
        std::string title;
        std::string upc;
        std::vector<std::string> isrcCodes;
    };
    bool exportDDP(const DDPConfig& config, const std::string& outputDir);

private:
    LoudnessMetrics m_metrics;
    SpectralProfile m_currentProfile;
    SpectralProfile m_targetProfile;
    bool m_hasTargetProfile;
    mutable std::mutex m_mutex;

    // Pre-allocated cache to ensure RT-safety and avoid mutex blockages
    std::array<float, 8> m_cachedGains;
    void* m_rustProcessor;
};

} // namespace Hirari::Core::Engine
