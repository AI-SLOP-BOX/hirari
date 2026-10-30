#include "../../external/signalsmith-stretch/signalsmith-stretch.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>

extern "C" bool hirari_signalsmith_stft_analyse_reference(
    const float* input, size_t block_size, size_t interval,
    float* spectrum_real, float* spectrum_imag, size_t spectrum_capacity,
    size_t* fft_size) {
    if (!input || !spectrum_real || !spectrum_imag || block_size < 2 ||
        block_size % 2 != 0 || interval == 0 || interval > block_size) return false;
    using Stft = signalsmith::linear::DynamicSTFT<
        float, false, signalsmith::linear::STFT_SPECTRUM_MODIFIED>;
    Stft stft;
    stft.configure(1, 1, block_size);
    stft.setInterval(interval, Stft::kaiser);
    if (spectrum_capacity < stft.bands()) return false;
    stft.writeInput(0, block_size, input);
    stft.analyse();
    const auto* spectrum = stft.spectrum(0);
    for (size_t bin = 0; bin < stft.bands(); ++bin) {
        spectrum_real[bin] = spectrum[bin].real();
        spectrum_imag[bin] = spectrum[bin].imag();
    }
    if (fft_size) *fft_size = stft.fftSamples();
    return true;
}

extern "C" bool hirari_signalsmith_real_fft_forward_reference(
    const float* input, size_t size, float* output_real, float* output_imag) {
    if (!input || !output_real || !output_imag || size < 2 || size % 2 != 0) return false;
    using Fft = signalsmith::linear::RealFFT<float, false, true>;
    Fft fft(size);
    std::vector<std::complex<float>> spectrum(size / 2);
    fft.fft(input, spectrum.data());
    for (size_t bin = 0; bin < spectrum.size(); ++bin) {
        output_real[bin] = spectrum[bin].real();
        output_imag[bin] = spectrum[bin].imag();
    }
    return true;
}

extern "C" bool hirari_signalsmith_real_fft_inverse_reference(
    const float* input_real, const float* input_imag, size_t size, float* output) {
    if (!input_real || !input_imag || !output || size < 2 || size % 2 != 0) return false;
    using Fft = signalsmith::linear::RealFFT<float, false, true>;
    Fft fft(size);
    std::vector<std::complex<float>> spectrum(size / 2);
    for (size_t bin = 0; bin < spectrum.size(); ++bin)
        spectrum[bin] = {input_real[bin], input_imag[bin]};
    fft.ifft(spectrum.data(), output);
    return true;
}

extern "C" bool hirari_signalsmith_kaiser_window_reference(
    size_t size, size_t interval, float* output) {
    if (!output || size < 2 || interval == 0 || interval > size) return false;
    using Stft = signalsmith::linear::DynamicSTFT<
        float, false, signalsmith::linear::STFT_SPECTRUM_MODIFIED>;
    Stft stft;
    stft.configure(1, 1, size, 0, interval);
    stft.setInterval(interval, Stft::kaiser);
    std::copy(stft.synthesisWindow(), stft.synthesisWindow() + size, output);
    return true;
}

namespace {
class RegionTimeStretchReference {
public:
    explicit RegionTimeStretchReference(long seed) : engine(seed) {}

    bool prepare(double sampleRate, uint32_t maxBlockSize) {
        if (!std::isfinite(sampleRate) || sampleRate < 8000.0 || maxBlockSize == 0) return false;
        if (prepared && this->sampleRate == sampleRate && this->maxBlockSize == maxBlockSize)
            return true;
        this->sampleRate = sampleRate;
        this->maxBlockSize = maxBlockSize;
        engine.presetDefault(2, static_cast<float>(sampleRate), true);
        inputCapacity = static_cast<size_t>(maxBlockSize) * 4u + 2u;
        inputLeft.resize(inputCapacity);
        inputRight.resize(inputCapacity);
        outputLeft.resize(maxBlockSize);
        outputRight.resize(maxBlockSize);
        const size_t maxSeek = static_cast<size_t>(std::max(
            engine.outputSeekLength(4.0f), engine.outputSeekLength(0.25f)));
        seekLeft.resize(maxSeek);
        seekRight.resize(maxSeek);
        prepared = true;
        reset();
        return true;
    }

    bool render(const float* sourceLeft, const float* sourceRight, uint64_t sourceSamples,
                uint64_t sourceOffset, uint64_t sourceSpan, uint64_t timelineOffset,
                double sourceStart, double sourceEnd, uint32_t outputOffset,
                uint32_t frames, bool reverse, float transpose, float formant,
                float formantBaseHz) noexcept {
        if (!prepared || !sourceLeft || !sourceRight || sourceSpan == 0 || frames == 0 ||
            frames > maxBlockSize || outputOffset > maxBlockSize ||
            frames > maxBlockSize - outputOffset || !std::isfinite(sourceStart) ||
            !std::isfinite(sourceEnd) || sourceStart < 0.0 || sourceEnd <= sourceStart ||
            sourceEnd > static_cast<double>(sourceSpan) || !std::isfinite(transpose) ||
            !std::isfinite(formant)) return false;
        engine.setTransposeSemitones(transpose);
        engine.setFormantSemitones(formant, true);
        if (std::isfinite(formantBaseHz) && formantBaseHz > 0.0f)
            engine.setFormantBase(formantBaseHz / static_cast<float>(sampleRate));
        const uint64_t inputStart = static_cast<uint64_t>(std::llround(sourceStart));
        const uint64_t inputEnd = static_cast<uint64_t>(std::llround(sourceEnd));
        if (inputEnd <= inputStart || inputEnd - inputStart > inputCapacity) return false;
        const size_t inputCount = static_cast<size_t>(inputEnd - inputStart);
        const double sourceRate = static_cast<double>(inputCount) / frames;
        if (!std::isfinite(sourceRate) || sourceRate < 0.25 || sourceRate > 4.0) return false;
        const bool canContinue = streamValid && nextTimelineOffset == timelineOffset &&
            nextInputOffset == inputStart && this->sourceLeft == sourceLeft &&
            this->sourceRight == sourceRight && this->sourceOffset == sourceOffset &&
            this->sourceSpan == sourceSpan && this->reverse == reverse;
        if (!canContinue && !seek(sourceLeft, sourceRight, sourceSamples, sourceOffset,
                                  sourceSpan, inputStart, sourceRate, reverse)) return false;
        for (size_t i = 0; i < inputCount; ++i) {
            const uint64_t relative = inputStart + i;
            inputLeft[i] = readSource(sourceLeft, sourceSamples, sourceOffset, sourceSpan,
                                      relative, reverse);
            inputRight[i] = readSource(sourceRight, sourceSamples, sourceOffset, sourceSpan,
                                       relative, reverse);
        }
        const float* inputs[2] = {inputLeft.data(), inputRight.data()};
        float* outputs[2] = {outputLeft.data() + outputOffset, outputRight.data() + outputOffset};
        engine.process(inputs, static_cast<int>(inputCount), outputs, static_cast<int>(frames));
        nextTimelineOffset = timelineOffset + frames;
        nextInputOffset = inputEnd;
        this->sourceLeft = sourceLeft;
        this->sourceRight = sourceRight;
        this->sourceOffset = sourceOffset;
        this->sourceSpan = sourceSpan;
        this->reverse = reverse;
        streamValid = true;
        return true;
    }

    const float* left() const { return outputLeft.data(); }
    const float* right() const { return outputRight.data(); }
    void reset() {
        engine.reset();
        streamValid = false;
        nextTimelineOffset = nextInputOffset = sourceOffset = sourceSpan = 0;
        sourceLeft = sourceRight = nullptr;
        reverse = false;
    }

private:
    static float readSource(const float* source, uint64_t sourceSamples,
                            uint64_t sourceOffset, uint64_t sourceSpan,
                            uint64_t relative, bool reverse) {
        if (relative >= sourceSpan) return 0.0f;
        const uint64_t index = reverse ? sourceOffset + sourceSpan - 1u - relative
                                       : sourceOffset + relative;
        if (index >= sourceSamples) return 0.0f;
        const float value = source[index];
        return std::isfinite(value) ? value : 0.0f;
    }

    bool seek(const float* left, const float* right, uint64_t sourceSamples,
              uint64_t sourceOffset, uint64_t sourceSpan, uint64_t inputPosition,
              double sourceRate, bool reverse) {
        const int samples = engine.outputSeekLength(static_cast<float>(sourceRate));
        if (samples <= 0 || static_cast<size_t>(samples) > seekLeft.size()) return false;
        const int64_t begin = static_cast<int64_t>(inputPosition) - samples;
        for (int i = 0; i < samples; ++i) {
            const int64_t relative = begin + i;
            if (relative < 0) {
                seekLeft[i] = seekRight[i] = 0.0f;
            } else {
                seekLeft[i] = readSource(left, sourceSamples, sourceOffset, sourceSpan,
                                         static_cast<uint64_t>(relative), reverse);
                seekRight[i] = readSource(right, sourceSamples, sourceOffset, sourceSpan,
                                          static_cast<uint64_t>(relative), reverse);
            }
        }
        const float* inputs[2] = {seekLeft.data(), seekRight.data()};
        engine.outputSeek(inputs, samples);
        nextInputOffset = inputPosition;
        streamValid = true;
        return true;
    }

    signalsmith::stretch::SignalsmithStretch<float> engine;
    std::vector<float> inputLeft, inputRight, outputLeft, outputRight, seekLeft, seekRight;
    size_t inputCapacity = 0;
    double sampleRate = 0.0;
    uint32_t maxBlockSize = 0;
    uint64_t nextTimelineOffset = 0, nextInputOffset = 0, sourceOffset = 0, sourceSpan = 0;
    const float* sourceLeft = nullptr;
    const float* sourceRight = nullptr;
    bool reverse = false, streamValid = false, prepared = false;
};
}

extern "C" void* hirari_region_stretch_reference_create() {
    return new RegionTimeStretchReference(17);
}
extern "C" void hirari_region_stretch_reference_destroy(void* handle) {
    delete static_cast<RegionTimeStretchReference*>(handle);
}
extern "C" bool hirari_region_stretch_reference_prepare(void* handle, double sampleRate,
                                                          uint32_t maxBlockSize) {
    return handle && static_cast<RegionTimeStretchReference*>(handle)->prepare(sampleRate,
                                                                               maxBlockSize);
}
extern "C" bool hirari_region_stretch_reference_render(
    void* handle, const float* sourceLeft, const float* sourceRight, uint64_t sourceSamples,
    uint64_t sourceOffset, uint64_t sourceSpan, uint64_t timelineOffset,
    double sourceStart, double sourceEnd, uint32_t outputOffset, uint32_t frames,
    bool reverse, float transpose, float formant, float formantBaseHz) {
    return handle && static_cast<RegionTimeStretchReference*>(handle)->render(
        sourceLeft, sourceRight, sourceSamples, sourceOffset, sourceSpan, timelineOffset,
        sourceStart, sourceEnd, outputOffset, frames, reverse, transpose, formant, formantBaseHz);
}
extern "C" const float* hirari_region_stretch_reference_left(const void* handle) {
    return handle ? static_cast<const RegionTimeStretchReference*>(handle)->left() : nullptr;
}
extern "C" const float* hirari_region_stretch_reference_right(const void* handle) {
    return handle ? static_cast<const RegionTimeStretchReference*>(handle)->right() : nullptr;
}
extern "C" void hirari_region_stretch_reference_reset(void* handle) {
    if (handle) static_cast<RegionTimeStretchReference*>(handle)->reset();
}
