#pragma once
#include <stdint.h>
#include <vector>
#include <mutex>
#include "../audio_buffer.hpp"

namespace Aura::Core::Engine {

struct SpectrogramFrame {
    uint64_t timestamp;
    float bins[256];
};

struct ForensicMetrics {
    float peak = 0.0f;
    float rms = 0.0f;
    float correlation = 0.0f;
    float lufsIntegrated = -180.0f;
};

class SignalForensicSuite {
public:
    SignalForensicSuite(uint32_t fftSize);
    
    void process(const AudioBuffer& buffer);
    void runFFT(const float* input, float* output);
    size_t getSpectrogram(SpectrogramFrame* out, size_t maxFrames);

private:
    uint32_t m_fftSize;
    ForensicMetrics m_currentMetrics;
    std::mutex m_mutex;
    std::vector<SpectrogramFrame> m_history;
};

} // namespace Aura::Core::Engine
