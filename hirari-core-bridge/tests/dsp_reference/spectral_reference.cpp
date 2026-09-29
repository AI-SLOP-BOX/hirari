// Frozen Hann-window STFT/OLA reference extracted from the pre-Rust
// SpectralProcessor::applySpectralGain implementation.
#include "dsp/utils/fft_utils.hpp"
#include <algorithm>
#include <bit>
#include <cmath>
#include <complex>
#include <cstdint>
#include <vector>

extern "C" bool spectral_reference_apply_gain(float* const* channels,
    uint32_t channel_count, uint32_t frames, double sample_rate,
    float t0, float f0, float t1, float f1, float gain) {
    if (!channels || channel_count == 0 || frames == 0 || !std::isfinite(sample_rate) ||
        sample_rate < 8000.0 || sample_rate > 384000.0 || !std::isfinite(gain) ||
        !std::isfinite(t0) || !std::isfinite(f0) || !std::isfinite(t1) || !std::isfinite(f1)) return false;
    if (t0 > t1) std::swap(t0, t1);
    if (f0 > f1) std::swap(f0, f1);
    t0 = std::max(0.0f, t0); t1 = std::max(t0, t1);
    f0 = std::max(0.0f, f0); f1 = std::max(f0, f1);
    gain = std::clamp(gain, 0.0f, 8.0f);
    constexpr uint32_t fft_size = 2048;
    constexpr uint32_t hop = fft_size / 2;
    std::vector<float> window(fft_size), original(fft_size);
    std::vector<std::complex<float>> spectrum(fft_size);
    std::vector<float> delta(frames), norm(frames);
    for (uint32_t i = 0; i < fft_size; ++i) {
        const float phase = static_cast<float>(i) / static_cast<float>(fft_size - 1);
        window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
    }
    const float frame_seconds = static_cast<float>(fft_size) / static_cast<float>(sample_rate);
    const float time_feather = std::max(frame_seconds * 0.5f, 1.0e-4f);
    const float bin_width = static_cast<float>(sample_rate) / static_cast<float>(fft_size);
    const float freq_feather = std::max(bin_width * 2.0f, 1.0e-3f);
    const auto smoothstep = [](float x) {
        x = std::clamp(x, 0.0f, 1.0f);
        return x * x * (3.0f - 2.0f * x);
    };
    for (uint32_t channel = 0; channel < channel_count; ++channel) {
        float* data = channels[channel];
        if (!data) continue;
        std::fill(delta.begin(), delta.end(), 0.0f);
        std::fill(norm.begin(), norm.end(), 0.0f);
        for (uint32_t offset = 0; offset < frames; offset += hop) {
            const float time = static_cast<float>(offset + fft_size / 2) / static_cast<float>(sample_rate);
            if (time < t0 - time_feather || time > t1 + time_feather) continue;
            const float time_weight = smoothstep((time - (t0 - time_feather)) / time_feather) *
                smoothstep(((t1 + time_feather) - time) / time_feather);
            if (time_weight <= 0.0f) continue;
            for (uint32_t i = 0; i < fft_size; ++i) {
                const uint32_t position = offset + i;
                const float sample = position < frames && std::isfinite(data[position]) ? data[position] : 0.0f;
                original[i] = sample * window[i];
                spectrum[i] = {original[i], 0.0f};
            }
            Hirari::DSP::Utils::FFTUtils::fft(spectrum);
            for (uint32_t k = 0; k < fft_size; ++k) {
                const float raw_frequency = static_cast<float>(k) * static_cast<float>(sample_rate) / static_cast<float>(fft_size);
                const float frequency = std::min(raw_frequency, static_cast<float>(sample_rate) - raw_frequency);
                if (frequency < f0 - freq_feather || frequency > f1 + freq_feather) continue;
                const float low = smoothstep((frequency - (f0 - freq_feather)) / freq_feather);
                const float high = smoothstep(((f1 + freq_feather) - frequency) / freq_feather);
                spectrum[k] *= 1.0f + (gain - 1.0f) * time_weight * low * high;
            }
            Hirari::DSP::Utils::FFTUtils::ifft(spectrum);
            for (uint32_t i = 0; i < fft_size; ++i) {
                const uint32_t position = offset + i;
                if (position >= frames) break;
                const float difference = (spectrum[i].real() - original[i]) * window[i];
                if (std::isfinite(difference)) delta[position] += difference;
                norm[position] += window[i] * window[i];
            }
        }
        for (uint32_t i = 0; i < frames; ++i) {
            const float difference = norm[i] > 1.0e-6f ? delta[i] / norm[i] : 0.0f;
            if (std::isfinite(difference)) data[i] += difference;
        }
    }
    return true;
}

extern "C" bool spectral_reference_fft(float* real, float* imag, size_t size, bool inverse) {
    if (!real || !imag || size < 2 || !std::has_single_bit(size)) return false;
    std::vector<std::complex<float>> spectrum(size);
    for (size_t i = 0; i < size; ++i) spectrum[i] = {real[i], imag[i]};
    if (inverse) Hirari::DSP::Utils::FFTUtils::ifft(spectrum);
    else Hirari::DSP::Utils::FFTUtils::fft(spectrum);
    for (size_t i = 0; i < size; ++i) { real[i] = spectrum[i].real(); imag[i] = spectrum[i].imag(); }
    return true;
}
