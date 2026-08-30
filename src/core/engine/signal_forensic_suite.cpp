#include "signal_forensic_suite.hpp"
#include "../audio_buffer.hpp"
#include <cmath>
#include <algorithm>
#include <cstring>

namespace Aura::Core::Engine {

SignalForensicSuite::SignalForensicSuite(uint32_t fftSize) 
    : m_fftSize(std::clamp(fftSize, 2u, 16384u))
{
    std::memset(&m_currentMetrics, 0, sizeof(m_currentMetrics));
    m_history.reserve(256);
}

/**
 * @brief PROCESS: Performs signal analysis with industrial precision and signal forensic sovereignty.
 * INDUSTRIAL: Delegating metering and phase correlation to the Rust 'SignalForensicOrchestrator'.
 */
void SignalForensicSuite::process(const AudioBuffer& buffer) {
    if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
    const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2u);
    const uint32_t samples = buffer.getNumSamples();
    float peak[2] = {0.0f, 0.0f};
    double energy[2] = {0.0, 0.0};
    double cross = 0.0;
    for (uint32_t i = 0; i < samples; ++i) {
        float values[2] = {0.0f, 0.0f};
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float* data = buffer.getReadPointer(channel);
            values[channel] = data && std::isfinite(data[i]) ? data[i] : 0.0f;
            peak[channel] = std::max(peak[channel], std::abs(values[channel]));
            energy[channel] += static_cast<double>(values[channel]) * values[channel];
        }
        if (channels == 2) cross += static_cast<double>(values[0]) * values[1];
    }

    SpectrogramFrame frame{};
    const float* input = buffer.getReadPointer(0);
    if (input != nullptr && samples >= m_fftSize) runFFT(input, frame.bins);

    std::lock_guard<std::mutex> lock(m_mutex);
    frame.timestamp = static_cast<uint64_t>(m_history.size());
    m_currentMetrics.peak = std::max(peak[0], channels > 1 ? peak[1] : peak[0]);
    const double norm = samples > 0 ? 1.0 / static_cast<double>(samples) : 0.0;
    const double denominator = std::sqrt(std::max(0.0, energy[0] * energy[1]));
    m_currentMetrics.correlation = channels > 1 && denominator > 1.0e-12
        ? static_cast<float>(std::clamp(cross / denominator, -1.0, 1.0)) : 0.0f;
    const double rms = std::sqrt(std::max(0.0, (energy[0] + energy[1]) * 0.5 * norm));
    m_currentMetrics.rms = static_cast<float>(rms);
    m_currentMetrics.lufsIntegrated = rms > 1.0e-9 ? 20.0f * std::log10(static_cast<float>(rms)) : -180.0f;
    if (m_history.size() == 256) m_history.erase(m_history.begin());
    m_history.push_back(frame);
}

void SignalForensicSuite::runFFT(const float* input, float* output) {
    if (input == nullptr || output == nullptr || m_fftSize < 2) return;
    constexpr double kPi = 3.1415926535897932384626433832795;
    const uint32_t bins = std::min<uint32_t>(256u, m_fftSize / 2u);
    for (uint32_t bin = 0; bin < bins; ++bin) {
        double real = 0.0;
        double imag = 0.0;
        for (uint32_t sample = 0; sample < m_fftSize; ++sample) {
            const float value = std::isfinite(input[sample]) ? input[sample] : 0.0f;
            const double phase = 2.0 * kPi * static_cast<double>(bin * sample) /
                                 static_cast<double>(m_fftSize);
            real += static_cast<double>(value) * std::cos(phase);
            imag -= static_cast<double>(value) * std::sin(phase);
        }
        output[bin] = static_cast<float>(std::sqrt(real * real + imag * imag) /
                                         static_cast<double>(m_fftSize));
    }
    for (uint32_t bin = bins; bin < 256u; ++bin) output[bin] = 0.0f;
}

size_t SignalForensicSuite::getSpectrogram(SpectrogramFrame* out, size_t maxFrames) {
    if (out == nullptr || maxFrames == 0) return 0;
    std::lock_guard<std::mutex> lock(m_mutex);
    size_t count = std::min(maxFrames, m_history.size());
    for (size_t i = 0; i < count; ++i) out[i] = m_history[i];
    return count;
}

} // namespace Aura::Core::Engine
