#include <cassert>
#include <filesystem>
#include <fstream>
#include <cmath>
#include <limits>
#include "dsp/analysis/stem_separator_provider.hpp"

int main() {
    using namespace Aura::DSP::Analysis;
    std::string error;

    StemSeparatorModelSpec invalid;
    invalid.config.backend = StemSeparatorBackend::OnnxRuntime;
    assert(!validateStemSeparatorModel(invalid, &error));
    assert(!error.empty());

    const auto path = std::filesystem::temp_directory_path() / "aura-test-model.onnx";
    { std::ofstream file(path, std::ios::binary); file << "test"; }
    StemSeparatorModelSpec valid;
    valid.config.backend = StemSeparatorBackend::OnnxRuntime;
    valid.config.modelPath = path.string();
    assert(validateStemSeparatorModel(valid, &error));

    Aura::Core::AudioBuffer input(2, 64);
    for (uint32_t index = 0; index < 64; ++index) {
        input.getWritePointer(0)[index] = static_cast<float>(index) / 64.0f;
        input.getWritePointer(1)[index] = static_cast<float>(index) / 64.0f;
    }
    StemSplitter::Stems stems;
    assert(StemSeparatorRegistry::instance().split(input, 48000.0, stems));
    assert(stems.vocals.getNumSamples() == 64);
    assert(stems.drums.getNumSamples() == 64);
    assert(stems.bass.getNumSamples() == 64);
    assert(stems.other.getNumSamples() == 64);
    const auto status = StemSeparatorRegistry::instance().status();
    assert(status.backend == StemSeparatorBackend::Heuristic);
    assert(status.available);
    assert(status.displayName == "Heuristic");

    Aura::Core::AudioBuffer multichannel(3, 16);
    for (uint32_t channel = 0; channel < 3; ++channel) {
        for (uint32_t index = 0; index < 16; ++index) {
            multichannel.getWritePointer(channel)[index] =
                (index == 3 && channel == 2)
                    ? std::numeric_limits<float>::quiet_NaN()
                    : static_cast<float>(channel + index) * 0.01f;
        }
    }
    StemSplitter::Stems multichannelStems;
    assert(StemSeparatorRegistry::instance().split(multichannel, 48'000.0, multichannelStems));
    for (const auto* output : {&multichannelStems.drums, &multichannelStems.bass,
                               &multichannelStems.vocals, &multichannelStems.other}) {
        assert(output->getNumChannels() == 3 && output->getNumSamples() == 16);
        for (uint32_t channel = 0; channel < 3; ++channel) {
            for (uint32_t index = 0; index < 16; ++index) {
                assert(std::isfinite(output->getReadPointer(channel)[index]));
            }
        }
    }

    std::filesystem::remove(path);
    return 0;
}
