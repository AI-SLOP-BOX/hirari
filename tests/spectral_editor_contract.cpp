#include "../src/dsp/analysis/spectral_editor.hpp"
#include "../src/dsp/analysis/fft_engine.hpp"

#include <cmath>
#include <cstdio>
#include <vector>

int main() {
    using Aura::DSP::Analysis::SpectralEditor;

    // Natural-order forward/inverse must be an identity before any editor
    // operation is trusted; this guards the permutation contract directly.
    Aura::DSP::Analysis::FFTEngine fft(16);
    std::array<float, 16> fftInput{};
    std::array<float, 16> fftOutput{};
    std::array<std::complex<float>, 16> spectrum{};
    for (uint32_t i = 0; i < fftInput.size(); ++i)
        fftInput[i] = std::sin(0.31f * static_cast<float>(i));
    fft.forward(fftInput, spectrum);
    fft.inverse(spectrum, fftOutput);
    for (uint32_t i = 0; i < fftInput.size(); ++i) {
        if (!std::isfinite(fftOutput[i]) || std::abs(fftOutput[i] - fftInput[i]) > 1.0e-4f)
            return 4;
    }

    constexpr uint32_t fftSize = 256;
    constexpr uint32_t samples = fftSize * 4;
    SpectralEditor editor(fftSize);
    std::vector<float> input(samples);
    std::vector<float> output(samples, 0.0f);
    for (uint32_t i = 0; i < samples; ++i)
        input[i] = 0.4f * std::sin(0.07f * static_cast<float>(i));

    // A host is allowed to submit a block larger than the analysis window.
    // This used to discard the just-produced OLA frame and return silence.
    editor.process(input.data(), output.data(), samples);
    editor.flush(output.data(), 0); // no-op must remain safe

    bool hasSignal = false;
    for (float sample : output) {
        if (!std::isfinite(sample)) return 1;
        if (std::abs(sample) > 1.0e-5f) hasSignal = true;
    }
    if (!hasSignal) {
        std::fprintf(stderr, "spectral editor discarded a full-size block\n");
        return 2;
    }

    // The same path must remain finite for a non-frame-aligned realtime block.
    editor.reset();
    output.assign(samples, 0.0f);
    editor.process(input.data(), output.data(), 127);
    for (uint32_t i = 0; i < 127; ++i)
        if (!std::isfinite(output[i])) return 3;
    return 0;
}
