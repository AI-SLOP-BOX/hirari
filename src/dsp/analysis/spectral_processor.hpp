#pragma once
#include <vector>
#include <deque>
#include <cstdint>
#include <string>
#include <iterator>
#include <cmath>
#include <complex>
#include "../utils/fft_utils.hpp"
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class SpectralProcessor
 * @brief iZotope RX / SpectralLayers style 2D Frequency Editor.
 * HONEST FIX: Implements Spectral Lasso and Region-specific processing.
 * Users can 'draw' on the spectrogram to isolate or remove specific 
 * frequencies at specific times.
 */
class SpectralProcessor {
public:
    struct Rect { float t0, f0, t1, f1; }; // Time (sec) / Freq (Hz)
    struct RegionGain { Rect region{}; float gain = 1.0f; };

    // Offline spectral edits are user operations, so keep a bounded history
    // that can be driven directly by a UI command or the stable API.
    bool canUndo() const noexcept { return !m_undo.empty(); }
    bool canRedo() const noexcept { return !m_redo.empty(); }
    bool canUndo(const Core::AudioBuffer& buffer) const noexcept {
        return findLastOwned(m_undo, &buffer) != m_undo.end();
    }
    bool canRedo(const Core::AudioBuffer& buffer) const noexcept {
        return findLastOwned(m_redo, &buffer) != m_redo.end();
    }
    size_t undoDepth() const noexcept { return m_undo.size(); }
    size_t redoDepth() const noexcept { return m_redo.size(); }
    size_t undoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return countOwned(m_undo, &buffer);
    }
    size_t redoDepth(const Core::AudioBuffer& buffer) const noexcept {
        return countOwned(m_redo, &buffer);
    }
    size_t historyBytes() const noexcept {
        size_t total = 0;
        for (const auto& item : m_undo) total += item.data.size() * sizeof(float);
        for (const auto& item : m_redo) total += item.data.size() * sizeof(float);
        return total;
    }
    const std::string& undoLabel() const noexcept {
        static const std::string empty;
        return m_undo.empty() ? empty : m_undo.back().label;
    }
    const std::string& redoLabel() const noexcept {
        static const std::string empty;
        return m_redo.empty() ? empty : m_redo.back().label;
    }
    const std::string& undoLabel(const Core::AudioBuffer& buffer) const noexcept {
        static const std::string empty;
        const auto it = findLastOwned(m_undo, &buffer);
        return it == m_undo.end() ? empty : it->label;
    }
    const std::string& redoLabel(const Core::AudioBuffer& buffer) const noexcept {
        static const std::string empty;
        const auto it = findLastOwned(m_redo, &buffer);
        return it == m_redo.end() ? empty : it->label;
    }

    bool undo(Core::AudioBuffer& buffer) {
        const auto it = findLastOwned(m_undo, &buffer);
        if (it == m_undo.end()) return false;
        Snapshot current;
        if (!makeSnapshot(buffer, current)) return false;
        current.label = it->label;
        if (!restoreSnapshot(buffer, *it)) return false;
        m_undo.erase(it);
        m_redo.push_back(std::move(current));
        trimHistory();
        return true;
    }

    bool redo(Core::AudioBuffer& buffer) {
        const auto it = findLastOwned(m_redo, &buffer);
        if (it == m_redo.end()) return false;
        Snapshot current;
        if (!makeSnapshot(buffer, current)) return false;
        current.label = it->label;
        if (!restoreSnapshot(buffer, *it)) return false;
        m_redo.erase(it);
        m_undo.push_back(std::move(current));
        trimHistory();
        return true;
    }

    void clearHistory() noexcept {
        m_undo.clear();
        m_redo.clear();
        m_groupHistory = false;
    }
    void clearHistory(const Core::AudioBuffer& buffer) noexcept {
        const auto owner = &buffer;
        std::erase_if(m_undo, [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        std::erase_if(m_redo, [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
    }

    void applyMask(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float gain) {
        applySpectralGain(buffer, sampleRate, target, gain);
    }

    // Non-destructive spectral gain edit. The inverse transform is accumulated
    // with a Hann window and only the edited delta is written back, preserving
    // phase continuity and samples outside the selection.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate, Rect target, float gain) {
        if (buffer.getNumSamples() == 0 || !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(gain)) return;
        if (!std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        if (!m_groupHistory) captureBefore(buffer, "Spectral Gain");
        if (target.t0 > target.t1) std::swap(target.t0, target.t1);
        if (target.f0 > target.f1) std::swap(target.f0, target.f1);
        target.t0 = std::max(0.0f, target.t0);
        target.t1 = std::max(target.t0, target.t1);
        target.f0 = std::max(0.0f, target.f0);
        target.f1 = std::max(target.f0, target.f1);
        gain = std::clamp(gain, 0.0f, 8.0f);

        const uint32_t fftSize = 2048;
        const uint32_t hop = fftSize / 2;
        std::vector<float> window(fftSize);
        std::vector<std::complex<float>> spectrum(fftSize);
        std::vector<float> originalTime(fftSize);
        std::vector<float> delta(buffer.getNumSamples(), 0.0f);
        std::vector<float> norm(buffer.getNumSamples(), 0.0f);
        for (uint32_t i = 0; i < fftSize; ++i) {
            const float phase = static_cast<float>(i) / static_cast<float>(fftSize - 1);
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
        }

        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            std::fill(delta.begin(), delta.end(), 0.0f);
            std::fill(norm.begin(), norm.end(), 0.0f);
            
            // Zero-pad edge frames so short clips and selections at the end of
            // a region are still editable (not silently skipped).
            const float frameSeconds = static_cast<float>(fftSize) / static_cast<float>(sampleRate);
            const float timeFeather = std::max(frameSeconds * 0.5f, 1.0e-4f);
            const float binWidth = static_cast<float>(sampleRate) / static_cast<float>(fftSize);
            const float freqFeather = std::max(binWidth * 2.0f, 1.0e-3f);
            const auto smoothstep = [](float x) noexcept {
                x = std::clamp(x, 0.0f, 1.0f);
                return x * x * (3.0f - 2.0f * x);
            };
            for (uint32_t offset = 0; offset < buffer.getNumSamples(); offset += hop) {
                const float timeSec = static_cast<float>(offset + fftSize / 2) / static_cast<float>(sampleRate);
                if (timeSec < target.t0 - timeFeather || timeSec > target.t1 + timeFeather) continue;
                const float timeWeight = smoothstep((timeSec - (target.t0 - timeFeather)) / timeFeather) *
                    smoothstep(((target.t1 + timeFeather) - timeSec) / timeFeather);
                if (timeWeight <= 0.0f) continue;

                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    const float sample = pos < buffer.getNumSamples() && std::isfinite(data[pos]) ? data[pos] : 0.0f;
                    originalTime[i] = sample * window[i];
                    spectrum[i] = {originalTime[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);

                for (uint32_t k = 0; k < fftSize; ++k) {
                    const float rawFreq = static_cast<float>(k) * static_cast<float>(sampleRate) / static_cast<float>(fftSize);
                    const float freq = std::min(rawFreq, static_cast<float>(sampleRate) - rawFreq);
                    if (freq < target.f0 - freqFeather || freq > target.f1 + freqFeather) continue;
                    const float lowWeight = smoothstep((freq - (target.f0 - freqFeather)) / freqFeather);
                    const float highWeight = smoothstep(((target.f1 + freqFeather) - freq) / freqFeather);
                    const float weight = timeWeight * lowWeight * highWeight;
                    spectrum[k] *= 1.0f + (gain - 1.0f) * weight;
                }
                
                Utils::FFTUtils::ifft(spectrum);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    if (pos >= buffer.getNumSamples()) break;
                    const float d = (spectrum[i].real() - originalTime[i]) * window[i];
                    if (std::isfinite(d)) delta[pos] += d;
                    norm[pos] += window[i] * window[i];
                }
            }
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float d = norm[i] > 1.0e-6f ? delta[i] / norm[i] : 0.0f;
                if (std::isfinite(d)) data[i] += d;
            }
        }
    }

    // Apply one spectral edit to a set of disjoint canvas regions.  UI lasso
    // and brush tools can rasterize their mask into rectangles and submit it
    // as one atomic operation, so undo restores the complete edit rather than
    // exposing one history item per raster cell.
    void applySpectralGain(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<Rect>& regions, float gain) {
        if (regions.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(gain)) return;
        bool hasValidRegion = false;
        for (const auto& region : regions) {
            if (std::isfinite(region.t0) && std::isfinite(region.t1) &&
                std::isfinite(region.f0) && std::isfinite(region.f1)) {
                hasValidRegion = true;
                break;
            }
        }
        if (!hasValidRegion) return;

        captureBefore(buffer, "Spectral Gain (Multi-Region)");
        m_groupHistory = true;
        try {
            for (const auto& region : regions)
                applySpectralGain(buffer, sampleRate, region, gain);
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
    }

    // Apply a variable-strength spectral brush mask. Each region can have a
    // distinct normalized gain (0=mute, 1=unchanged, >1=boost), while the
    // entire brush stroke remains one atomic undo operation.
    void applySpectralMask(Core::AudioBuffer& buffer, double sampleRate,
                           const std::vector<RegionGain>& mask) {
        if (mask.empty() || buffer.getNumSamples() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0) return;
        bool hasValidRegion = false;
        for (const auto& item : mask) {
            if (std::isfinite(item.region.t0) && std::isfinite(item.region.t1) &&
                std::isfinite(item.region.f0) && std::isfinite(item.region.f1) &&
                std::isfinite(item.gain)) {
                hasValidRegion = true;
                break;
            }
        }
        if (!hasValidRegion) return;
        captureBefore(buffer, "Spectral Brush");
        m_groupHistory = true;
        try {
            for (const auto& item : mask) {
                if (std::isfinite(item.region.t0) && std::isfinite(item.region.t1) &&
                    std::isfinite(item.region.f0) && std::isfinite(item.region.f1) &&
                    std::isfinite(item.gain))
                    applySpectralGain(buffer, sampleRate, item.region, item.gain);
            }
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
    }

    // Remove mains hum and its harmonics in a selected time range.
    void removeHum(Core::AudioBuffer& buffer, double sampleRate, float fundamentalHz,
                   uint32_t harmonics = 8, float bandwidthHz = 3.0f) {
        removeHum(buffer, sampleRate, fundamentalHz, Rect{0.0f, 0.0f, 1.0e9f,
                                                           static_cast<float>(sampleRate * 0.5)},
                  harmonics, bandwidthHz);
    }

    // Remove mains hum only inside a selected time/frequency canvas region.
    // The frequency bounds are intersected with each harmonic band, allowing
    // a UI lasso to leave unrelated low-frequency content untouched.
    void removeHum(Core::AudioBuffer& buffer, double sampleRate, float fundamentalHz,
                   Rect selection, uint32_t harmonics = 8,
                   float bandwidthHz = 3.0f) {
        if (!std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(fundamentalHz) || fundamentalHz <= 0.0f ||
            !std::isfinite(bandwidthHz) || bandwidthHz <= 0.0f || harmonics == 0) return;
        if (!std::isfinite(selection.t0) || !std::isfinite(selection.t1) ||
            !std::isfinite(selection.f0) || !std::isfinite(selection.f1)) return;
        if (buffer.getNumSamples() == 0 || buffer.getNumChannels() == 0) return;
        if (selection.t0 > selection.t1) std::swap(selection.t0, selection.t1);
        if (selection.f0 > selection.f1) std::swap(selection.f0, selection.f1);
        selection.t0 = std::max(0.0f, selection.t0);
        selection.t1 = std::max(selection.t0, selection.t1);
        selection.f0 = std::max(0.0f, selection.f0);
        selection.f1 = std::max(selection.f0, selection.f1);
        captureBefore(buffer, "Remove Hum");
        m_groupHistory = true;
        const float nyquist = static_cast<float>(sampleRate * 0.5);
        try {
            for (uint32_t h = 1; h <= std::min<uint32_t>(harmonics, 32u); ++h) {
                const float f = fundamentalHz * static_cast<float>(h);
                if (f >= nyquist) break;
                const float low = std::max(selection.f0, f - bandwidthHz);
                const float high = std::min(selection.f1, f + bandwidthHz);
                if (high > low) applySpectralGain(buffer, sampleRate,
                                                   Rect{selection.t0, low, selection.t1, high}, 0.0f);
            }
        } catch (...) {
            m_groupHistory = false;
            throw;
        }
        m_groupHistory = false;
    }

    // Learn a stationary noise bed from the leading portion of a clip and
    // apply bounded spectral subtraction. When target carries a finite time
    // or frequency range, only that Spectral Canvas selection is edited. This
    // is an offline restoration operation and keeps the original phase.
    void reduceNoise(Core::AudioBuffer& buffer, double sampleRate,
                     float amount = 1.0f, float profileSeconds = 0.5f,
                     Rect target = Rect{0.0f, 0.0f, 1.0e9f, 0.0f}) {
        if (buffer.getNumSamples() < 64 || buffer.getNumChannels() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0) return;
        amount = std::clamp(std::isfinite(amount) ? amount : 1.0f, 0.0f, 1.0f);
        profileSeconds = std::clamp(std::isfinite(profileSeconds) ? profileSeconds : 0.5f,
                                    0.02f, 10.0f);
        if (!std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        if (target.t0 > target.t1) std::swap(target.t0, target.t1);
        if (target.f0 > target.f1) std::swap(target.f0, target.f1);
        target.t0 = std::max(0.0f, target.t0);
        target.t1 = std::max(target.t0, target.t1);
        target.f0 = std::max(0.0f, target.f0);
        target.f1 = std::max(target.f0, target.f1);
        captureBefore(buffer, "Reduce Noise");
        const uint32_t fftSize = 2048;
        const uint32_t hop = fftSize / 2;
        const float timeFeather = std::max(static_cast<float>(fftSize) /
                                           static_cast<float>(sampleRate) * 0.5f, 1.0e-4f);
        const float binWidth = static_cast<float>(sampleRate) / static_cast<float>(fftSize);
        const float freqFeather = std::max(binWidth * 2.0f, 1.0e-3f);
        const bool restrictFrequency = target.f1 > target.f0;
        const auto smoothstep = [](float x) noexcept {
            x = std::clamp(x, 0.0f, 1.0f);
            return x * x * (3.0f - 2.0f * x);
        };
        std::vector<float> window(fftSize);
        std::vector<std::complex<float>> spectrum(fftSize);
        std::vector<float> profile(fftSize, 0.0f);
        for (uint32_t i = 0; i < fftSize; ++i) {
            const float phase = static_cast<float>(i) / static_cast<float>(fftSize - 1);
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
        }
        const uint32_t profileFrames = std::max<uint32_t>(1u,
            static_cast<uint32_t>(profileSeconds * sampleRate / hop));
        // Build one linked noise profile from all available channels. Applying
        // a common floor keeps stereo/immersive image stable instead of
        // modulating each channel independently.
        uint32_t learned = 0;
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            const float* data = buffer.getReadPointer(c);
            if (!data) continue;
            uint32_t channelFrames = 0;
            for (uint32_t offset = 0; offset < buffer.getNumSamples() && channelFrames < profileFrames; offset += hop, ++channelFrames) {
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    const float sample = pos < buffer.getNumSamples() && std::isfinite(data[pos]) ? data[pos] : 0.0f;
                    spectrum[i] = {sample * window[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);
                for (uint32_t k = 0; k < fftSize; ++k) profile[k] += std::abs(spectrum[k]);
            }
            learned += channelFrames;
        }
        if (learned == 0) return;
        for (float& value : profile) value /= static_cast<float>(learned);
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            const float* data = buffer.getReadPointer(c);
            if (!data) continue;
            std::vector<float> delta(buffer.getNumSamples(), 0.0f);
            std::vector<float> norm(buffer.getNumSamples(), 0.0f);
            for (uint32_t offset = 0; offset < buffer.getNumSamples(); offset += hop) {
                const float timeSec = static_cast<float>(offset + fftSize / 2) /
                                      static_cast<float>(sampleRate);
                if (timeSec < target.t0 - timeFeather || timeSec > target.t1 + timeFeather) continue;
                const float timeWeight = smoothstep((timeSec - (target.t0 - timeFeather)) / timeFeather) *
                    smoothstep(((target.t1 + timeFeather) - timeSec) / timeFeather);
                if (timeWeight <= 0.0f) continue;
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    const float sample = pos < buffer.getNumSamples() && std::isfinite(data[pos]) ? data[pos] : 0.0f;
                    spectrum[i] = {sample * window[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);
                for (uint32_t k = 0; k < fftSize; ++k) {
                    const float magnitude = std::abs(spectrum[k]);
                    if (magnitude <= 1.0e-9f) continue;
                    float frequencyWeight = 1.0f;
                    if (restrictFrequency) {
                        const float rawFrequency = static_cast<float>(k) * binWidth;
                        const float frequency = std::min(rawFrequency,
                                                         static_cast<float>(sampleRate) - rawFrequency);
                        if (frequency < target.f0 - freqFeather || frequency > target.f1 + freqFeather) continue;
                        const float low = smoothstep((frequency - (target.f0 - freqFeather)) / freqFeather);
                        const float high = smoothstep(((target.f1 + freqFeather) - frequency) / freqFeather);
                        frequencyWeight = low * high;
                    }
                    const float subtraction = std::min(magnitude * 0.98f,
                                                        profile[k] * amount * timeWeight * frequencyWeight);
                    spectrum[k] *= (magnitude - subtraction) / magnitude;
                }
                Utils::FFTUtils::ifft(spectrum);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    if (pos >= buffer.getNumSamples()) break;
                    delta[pos] += spectrum[i].real() * window[i];
                    norm[pos] += window[i] * window[i];
                }
            }
            float* writable = buffer.getWritePointer(c);
            if (!writable) continue;
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float value = norm[i] > 1.0e-6f ? delta[i] / norm[i] : writable[i];
                if (std::isfinite(value)) writable[i] = value;
            }
        }
    }

    // Heal a time/frequency selection by attenuating it with a smooth spectral
    // feather. This is intentionally bounded and click-free for offline edits.
    void healRegion(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float amount = 1.0f) {
        amount = std::clamp(std::isfinite(amount) ? amount : 1.0f, 0.0f, 1.0f);
        applySpectralGain(buffer, sampleRate, target, 1.0f - amount);
    }

    // Spectral inpainting: reconstruct a selected band from the nearest
    // unselected bins while retaining each bin's original phase. A short
    // Hann-windowed OLA keeps the repair continuous at the selection edges.
    void interpolateRegion(Core::AudioBuffer& buffer, double sampleRate,
                           Rect target, float blend = 1.0f) {
        if (buffer.getNumSamples() < 64 || buffer.getNumChannels() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0) return;
        if (!std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        captureBefore(buffer, "Interpolate Region");
        if (target.t0 > target.t1) std::swap(target.t0, target.t1);
        if (target.f0 > target.f1) std::swap(target.f0, target.f1);
        target.t0 = std::max(0.0f, target.t0);
        target.t1 = std::max(target.t0, target.t1);
        target.f0 = std::max(0.0f, target.f0);
        target.f1 = std::max(target.f0, target.f1);
        blend = std::clamp(std::isfinite(blend) ? blend : 1.0f, 0.0f, 1.0f);
        const uint32_t fftSize = 2048, hop = fftSize / 2;
        const float timeFeather = std::max(static_cast<float>(fftSize) /
                                           static_cast<float>(sampleRate) * 0.5f, 1.0e-4f);
        const float freqFeather = std::max((static_cast<float>(sampleRate) /
                                            static_cast<float>(fftSize)) * 2.0f, 1.0e-3f);
        const auto smoothstep = [](float x) noexcept {
            x = std::clamp(x, 0.0f, 1.0f);
            return x * x * (3.0f - 2.0f * x);
        };
        std::vector<float> window(fftSize), delta(buffer.getNumSamples(), 0.0f), norm(buffer.getNumSamples(), 0.0f);
        std::vector<std::complex<float>> spectrum(fftSize), previousSpectrum(fftSize);
        std::vector<float> originalTime(fftSize);
        bool havePreviousSpectrum = false;
        for (uint32_t i = 0; i < fftSize; ++i)
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * static_cast<float>(i) / static_cast<float>(fftSize - 1));
        const float binWidth = static_cast<float>(sampleRate) / static_cast<float>(fftSize);
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            const float* input = buffer.getReadPointer(c);
            float* output = buffer.getWritePointer(c);
            if (!input || !output) continue;
            // Temporal interpolation state is channel-local; carrying the
            // previous frame from channel 0 into channel 1 would smear stereo
            // phase and defeat linked-but-independent restoration.
            std::fill(previousSpectrum.begin(), previousSpectrum.end(), std::complex<float>{0.0f, 0.0f});
            havePreviousSpectrum = false;
            std::fill(delta.begin(), delta.end(), 0.0f);
            std::fill(norm.begin(), norm.end(), 0.0f);
            for (uint32_t offset = 0; offset < buffer.getNumSamples(); offset += hop) {
                const float timeSec = static_cast<float>(offset + fftSize / 2) / static_cast<float>(sampleRate);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    const float sample = pos < buffer.getNumSamples() && std::isfinite(input[pos]) ? input[pos] : 0.0f;
                    originalTime[i] = sample * window[i];
                    spectrum[i] = {originalTime[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);
                // Keep an untouched neighboring frame for full-band repairs.
                // Using DC/Nyquist as interpolation anchors produces a false
                // ramp when the selection spans the entire frequency axis.
                std::copy(spectrum.begin(), spectrum.end(), previousSpectrum.begin());
                if (timeSec >= target.t0 - timeFeather && timeSec <= target.t1 + timeFeather) {
                    const float timeWeight = smoothstep((timeSec - (target.t0 - timeFeather)) / timeFeather) *
                        smoothstep(((target.t1 + timeFeather) - timeSec) / timeFeather);
                    const int32_t first = std::max<int32_t>(0, static_cast<int32_t>(std::floor(target.f0 / binWidth)));
                    const int32_t last = std::min<int32_t>(static_cast<int32_t>(fftSize / 2), static_cast<int32_t>(std::ceil(target.f1 / binWidth)));
                    const int32_t left = first > 0 ? first - 1 : std::min<int32_t>(last + 1, static_cast<int32_t>(fftSize / 2));
                    const int32_t right = last < static_cast<int32_t>(fftSize / 2) ? last + 1 : std::max<int32_t>(first - 1, 0);
                    const bool fullBand = first == 0 && last == static_cast<int32_t>(fftSize / 2);
                    for (int32_t k = first; k <= last; ++k) {
                        const int32_t mirror = static_cast<int32_t>(fftSize) - k;
                        const float alpha = last > first ? static_cast<float>(k - first) / static_cast<float>(last - first) : 0.5f;
                        const std::complex<float> estimate = fullBand && havePreviousSpectrum
                            ? previousSpectrum[k]
                            : (spectrum[left] * (1.0f - alpha) + spectrum[right] * alpha);
                        const float frequency = static_cast<float>(k) * binWidth;
                        const float lowWeight = smoothstep((frequency - (target.f0 - freqFeather)) / freqFeather);
                        const float highWeight = smoothstep(((target.f1 + freqFeather) - frequency) / freqFeather);
                        const float effectiveBlend = blend * timeWeight * lowWeight * highWeight;
                        spectrum[k] = spectrum[k] * (1.0f - effectiveBlend) + estimate * effectiveBlend;
                        if (mirror > 0 && mirror < static_cast<int32_t>(fftSize)) spectrum[mirror] = std::conj(spectrum[k]);
                    }
                }
                havePreviousSpectrum = true;
                Utils::FFTUtils::ifft(spectrum);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const uint32_t pos = offset + i;
                    if (pos >= buffer.getNumSamples()) break;
                    delta[pos] += (spectrum[i].real() - originalTime[i]) * window[i];
                    norm[pos] += window[i] * window[i];
                }
            }
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float d = norm[i] > 1.0e-6f ? delta[i] / norm[i] : 0.0f;
                if (std::isfinite(d)) output[i] = std::isfinite(output[i]) ? output[i] + d : d;
            }
        }
    }

    // Detect isolated discontinuities and replace them with a short
    // equal-power interpolation. This is deliberately channel-local so a
    // click in one microphone does not alter the phase of the other channel.
    uint32_t removeClicks(Core::AudioBuffer& buffer, float threshold = 0.65f,
                          uint32_t radius = 8) {
        threshold = std::clamp(std::isfinite(threshold) ? threshold : 0.65f, 0.01f, 1.0f);
        radius = std::clamp<uint32_t>(radius, 1u, 64u);
        uint32_t repaired = 0;
        const uint32_t n = buffer.getNumSamples();
        if (n < radius * 2u + 1u) return 0;
        captureBefore(buffer, "Remove Clicks");
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            for (uint32_t i = radius; i + radius < n; ++i) {
                const float x = data[i];
                if (!std::isfinite(x)) { data[i] = 0.0f; ++repaired; continue; }
                const float left = data[i - radius];
                const float right = data[i + radius];
                const float baseline = 0.5f * (left + right);
                if (std::fabs(x - baseline) <= threshold ||
                    std::fabs(x - left) <= threshold || std::fabs(x - right) <= threshold) continue;
                for (uint32_t j = 0; j <= radius * 2u; ++j) {
                    const float t = static_cast<float>(j) / static_cast<float>(radius * 2u);
                    const float s = t * t * (3.0f - 2.0f * t);
                    data[i - radius + j] = left + (right - left) * s;
                }
                ++repaired;
                i += radius;
            }
        }
        return repaired;
    }

    // Click removal constrained to a time/frequency canvas selection.  Clicks
    // are time-domain events, so the frequency bounds are intentionally
    // ignored; the time bounds are sample-accurate and the interpolation
    // window is clipped to the selected interval.
    uint32_t removeClicks(Core::AudioBuffer& buffer, double sampleRate,
                          Rect selection, float threshold = 0.65f,
                          uint32_t radius = 8) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() < 3 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(selection.t0) ||
            !std::isfinite(selection.t1)) return 0;
        if (selection.t0 > selection.t1) std::swap(selection.t0, selection.t1);
        selection.t0 = std::max(0.0f, selection.t0);
        selection.t1 = std::max(selection.t0, selection.t1);
        threshold = std::clamp(std::isfinite(threshold) ? threshold : 0.65f, 0.01f, 1.0f);
        radius = std::clamp<uint32_t>(radius, 1u, 64u);
        const uint32_t n = buffer.getNumSamples();
        radius = std::min<uint32_t>(radius, (n - 1u) / 2u);
        if (radius == 0) return 0;
        const float duration = static_cast<float>(static_cast<double>(n) / sampleRate);
        selection.t0 = std::min(selection.t0, duration);
        selection.t1 = std::min(selection.t1, duration);
        const uint32_t first = std::max<uint32_t>(radius,
            static_cast<uint32_t>(std::ceil(selection.t0 * sampleRate)));
        const uint64_t selectedLast = static_cast<uint64_t>(std::floor(selection.t1 * sampleRate));
        const uint32_t last = static_cast<uint32_t>(std::min<uint64_t>(n - radius - 1u, selectedLast));
        if (first > last) return 0;
        captureBefore(buffer, "Remove Clicks (Selection)");
        uint32_t repaired = 0;
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            for (uint32_t i = first; i <= last; ++i) {
                const float x = data[i];
                if (!std::isfinite(x)) { data[i] = 0.0f; ++repaired; continue; }
                const float left = data[i - radius];
                const float right = data[i + radius];
                const float baseline = 0.5f * (left + right);
                if (std::fabs(x - baseline) <= threshold ||
                    std::fabs(x - left) <= threshold || std::fabs(x - right) <= threshold) continue;
                const uint32_t begin = std::max<uint32_t>(i - radius, first);
                const uint32_t end = std::min<uint32_t>(i + radius, last);
                if (end <= begin) continue;
                for (uint32_t j = begin; j <= end; ++j) {
                    const float t = static_cast<float>(j - begin) /
                                    static_cast<float>(end - begin);
                    const float s = t * t * (3.0f - 2.0f * t);
                    data[j] = left + (right - left) * s;
                }
                ++repaired;
                i = std::min<uint32_t>(last, i + radius);
            }
        }
        return repaired;
    }

    // Reconstruct runs that hit a digital ceiling. The surrounding samples
    // define the slope, while the run itself is filled with a smooth cubic
    // curve to avoid introducing a new discontinuity.
    uint32_t repairClipped(Core::AudioBuffer& buffer, float ceiling = 0.999f) {
        ceiling = std::clamp(std::isfinite(ceiling) ? std::fabs(ceiling) : 0.999f, 0.5f, 1.0f);
        const uint32_t n = buffer.getNumSamples();
        uint32_t repaired = 0;
        if (n > 0 && buffer.getNumChannels() > 0) captureBefore(buffer, "Repair Clipped");
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data || n < 3) continue;
            uint32_t i = 1;
            while (i + 1 < n) {
                if (!std::isfinite(data[i]) || std::fabs(data[i]) < ceiling) { ++i; continue; }
                const uint32_t start = i;
                while (i + 1 < n && std::isfinite(data[i]) && std::fabs(data[i]) >= ceiling) ++i;
                const uint32_t end = i;
                if (start == 0 || end >= n || end <= start) continue;
                const float a = data[start - 1];
                const float b = data[end];
                const uint32_t span = end - start + 1u;
                for (uint32_t j = 0; j < span; ++j) {
                    const float t = static_cast<float>(j + 1u) / static_cast<float>(span + 1u);
                    const float s = t * t * (3.0f - 2.0f * t);
                    data[start + j] = a + (b - a) * s;
                }
                repaired += end - start;
            }
        }
        return repaired;
    }

    // Repair clipped runs only inside a selected time interval.  A run is
    // accepted only when both its surrounding samples are also inside the
    // selection, preventing a surgical edit from changing adjacent audio.
    uint32_t repairClipped(Core::AudioBuffer& buffer, double sampleRate,
                           Rect selection, float ceiling = 0.999f) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() < 3 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || !std::isfinite(selection.t0) ||
            !std::isfinite(selection.t1)) return 0;
        if (selection.t0 > selection.t1) std::swap(selection.t0, selection.t1);
        selection.t0 = std::max(0.0f, selection.t0);
        selection.t1 = std::max(selection.t0, selection.t1);
        ceiling = std::clamp(std::isfinite(ceiling) ? std::fabs(ceiling) : 0.999f, 0.5f, 1.0f);
        const uint32_t n = buffer.getNumSamples();
        const float duration = static_cast<float>(static_cast<double>(n) / sampleRate);
        selection.t0 = std::min(selection.t0, duration);
        selection.t1 = std::min(selection.t1, duration);
        const uint32_t first = std::max<uint32_t>(1u,
            static_cast<uint32_t>(std::ceil(selection.t0 * sampleRate)));
        const uint64_t selectedLast = static_cast<uint64_t>(std::floor(selection.t1 * sampleRate));
        const uint32_t last = static_cast<uint32_t>(std::min<uint64_t>(n - 2u, selectedLast));
        if (first > last) return 0;
        captureBefore(buffer, "Repair Clipped (Selection)");
        uint32_t repaired = 0;
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            uint32_t i = first;
            while (i <= last) {
                if (!std::isfinite(data[i]) || std::fabs(data[i]) < ceiling) { ++i; continue; }
                const uint32_t start = i;
                while (i <= last && std::isfinite(data[i]) && std::fabs(data[i]) >= ceiling) ++i;
                const uint32_t end = i;
                if (start == first || end > last || end <= start) continue;
                const float a = data[start - 1u];
                const float b = data[end];
                const uint32_t span = end - start;
                for (uint32_t j = 0; j < span; ++j) {
                    const float t = static_cast<float>(j + 1u) / static_cast<float>(span + 1u);
                    const float s = t * t * (3.0f - 2.0f * t);
                    data[start + j] = a + (b - a) * s;
                }
                repaired += span;
            }
        }
        return repaired;
    }
private:
    struct Snapshot {
        const Core::AudioBuffer* owner = nullptr;
        uint32_t channels = 0;
        uint32_t samples = 0;
        std::string label;
        std::vector<float> data;
    };

    static constexpr size_t kMaxHistory = 16;
    static constexpr size_t kMaxSnapshotSamples = 16u * 1024u * 1024u;
    static constexpr size_t kMaxHistoryBytes = 256u * 1024u * 1024u;

    using History = std::deque<Snapshot>;
    static History::const_iterator findLastOwned(const History& history,
                                                  const Core::AudioBuffer* owner) noexcept {
        const auto reverse = std::find_if(history.rbegin(), history.rend(),
                                          [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        return reverse == history.rend() ? history.end() : std::prev(reverse.base());
    }
    static History::iterator findLastOwned(History& history,
                                           const Core::AudioBuffer* owner) noexcept {
        const auto reverse = std::find_if(history.rbegin(), history.rend(),
                                          [owner](const Snapshot& snapshot) { return snapshot.owner == owner; });
        return reverse == history.rend() ? history.end() : std::prev(reverse.base());
    }
    static size_t countOwned(const History& history,
                             const Core::AudioBuffer* owner) noexcept {
        return static_cast<size_t>(std::count_if(
            history.begin(), history.end(),
            [owner](const Snapshot& snapshot) { return snapshot.owner == owner; }));
    }

    static bool makeSnapshot(const Core::AudioBuffer& buffer, Snapshot& out) {
        const uint64_t count = static_cast<uint64_t>(buffer.getNumChannels()) * buffer.getNumSamples();
        if (count > kMaxSnapshotSamples) return false;
        out.channels = buffer.getNumChannels();
        out.samples = buffer.getNumSamples();
        out.owner = &buffer;
        try { out.data.resize(static_cast<size_t>(count)); }
        catch (...) { return false; }
        size_t at = 0;
        for (uint32_t c = 0; c < out.channels; ++c) {
            const float* source = buffer.getReadPointer(c);
            if (!source) return false;
            std::copy(source, source + out.samples, out.data.begin() + static_cast<std::ptrdiff_t>(at));
            at += out.samples;
        }
        return true;
    }

    static bool restoreSnapshot(Core::AudioBuffer& buffer, const Snapshot& snapshot) {
        if (snapshot.channels == 0 || snapshot.samples == 0 ||
            snapshot.data.size() != static_cast<size_t>(snapshot.channels) * snapshot.samples) return false;
        if (buffer.getNumChannels() != snapshot.channels || buffer.getNumSamples() != snapshot.samples) {
            if (!buffer.resize(snapshot.channels, snapshot.samples)) return false;
        }
        size_t at = 0;
        for (uint32_t c = 0; c < snapshot.channels; ++c) {
            float* destination = buffer.getWritePointer(c);
            if (!destination) return false;
            std::copy(snapshot.data.begin() + static_cast<std::ptrdiff_t>(at),
                      snapshot.data.begin() + static_cast<std::ptrdiff_t>(at + snapshot.samples), destination);
            at += snapshot.samples;
        }
        return true;
    }

    void captureBefore(const Core::AudioBuffer& buffer, const char* label) {
        Snapshot snapshot;
        if (!makeSnapshot(buffer, snapshot)) return;
        snapshot.label = label ? label : "Spectral Edit";
        if (m_undo.size() >= kMaxHistory) m_undo.pop_front();
        m_undo.push_back(std::move(snapshot));
        m_redo.clear();
        trimHistory();
    }

    void trimHistory() noexcept {
        while (historyBytes() > kMaxHistoryBytes && (!m_undo.empty() || !m_redo.empty())) {
            if (!m_undo.empty()) m_undo.pop_front();
            else m_redo.pop_front();
        }
    }

    History m_undo;
    History m_redo;
    bool m_groupHistory = false;
};

} // namespace Aura::DSP::Analysis
