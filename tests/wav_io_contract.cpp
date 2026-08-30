#include <cassert>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <limits>
#include <vector>
#include "io/wav_loader_utils.hpp"
#include "io/wav_reader.hpp"
#include "core/io/audio_decoder.hpp"

int main() {
    const auto invalidPath = std::filesystem::temp_directory_path() / "aura-wav-invalid-input-contract.wav";
    assert(!Aura::IO::WavSaver::save(invalidPath.string(), {{}}, 48000));
    assert(!Aura::IO::WavSaver::save(invalidPath.string(), {{0.0f}, {0.0f, 1.0f}}, 48000));

    const auto path = std::filesystem::temp_directory_path() / "aura-wav-contract.wav";
    std::vector<std::vector<float>> source(2, std::vector<float>(4));
    source[0] = {0.0f, 0.5f, std::numeric_limits<float>::quiet_NaN(), -1.2f};
    source[1] = {0.0f, -0.5f, std::numeric_limits<float>::infinity(), 1.2f};
    assert(Aura::IO::WavSaver::save(path.string(), source, 48000));

    Aura::IO::WavLoader::WavInfo info{};
    const auto decoded = Aura::IO::WavLoader::load(path.string(), info);
    assert(info.numChannels == 2);
    assert(info.numSamples == 4);
    for (const auto& channel : decoded) {
        for (float sample : channel) assert(std::isfinite(sample));
    }

    Aura::Core::IO::WavDecoder canonical;
    assert(canonical.open(path.string()));
    Aura::Core::AudioBuffer canonicalBuffer;
    canonical.decodeFull(canonicalBuffer);
    assert(canonicalBuffer.getNumChannels() == 2);
    assert(canonicalBuffer.getNumSamples() == 4);
    for (uint32_t channel = 0; channel < canonicalBuffer.getNumChannels(); ++channel)
        for (uint32_t sample = 0; sample < canonicalBuffer.getNumSamples(); ++sample)
            assert(std::isfinite(canonicalBuffer.getReadPointer(channel)[sample]));

    Aura::IO::WavReader reader(path.string());
    assert(reader.getNumChannels() == 2);
    assert(reader.getNumSamples() == 4);
    std::filesystem::remove(path);

    const auto oddChunk = std::filesystem::temp_directory_path() / "aura-wav-odd-chunk-contract.wav";
    {
        std::ofstream file(oddChunk, std::ios::binary);
        const uint32_t riffSize = 52;
        const uint32_t junkSize = 3;
        const uint32_t fmtSize = 16;
        const uint32_t dataSize = 2;
        const uint16_t pcm = 1, channels = 1, bits = 16, align = 2;
        const uint32_t sampleRate = 48000, byteRate = 96000;
        const int16_t sample = 16384;
        file.write("RIFF", 4); file.write(reinterpret_cast<const char*>(&riffSize), 4);
        file.write("WAVE", 4);
        file.write("JUNK", 4); file.write(reinterpret_cast<const char*>(&junkSize), 4);
        file.write("abc", 3); file.put('\0');
        file.write("fmt ", 4); file.write(reinterpret_cast<const char*>(&fmtSize), 4);
        file.write(reinterpret_cast<const char*>(&pcm), 2);
        file.write(reinterpret_cast<const char*>(&channels), 2);
        file.write(reinterpret_cast<const char*>(&sampleRate), 4);
        file.write(reinterpret_cast<const char*>(&byteRate), 4);
        file.write(reinterpret_cast<const char*>(&align), 2);
        file.write(reinterpret_cast<const char*>(&bits), 2);
        file.write("data", 4); file.write(reinterpret_cast<const char*>(&dataSize), 4);
        file.write(reinterpret_cast<const char*>(&sample), 2);
    }
    Aura::Core::IO::WavDecoder oddDecoder;
    assert(oddDecoder.open(oddChunk.string()));
    Aura::Core::AudioBuffer oddBuffer;
    oddDecoder.decodeFull(oddBuffer);
    assert(oddBuffer.getNumChannels() == 1 && oddBuffer.getNumSamples() == 1);
    assert(std::abs(oddBuffer.getReadPointer(0)[0] - 0.5f) < 1.0e-5f);
    std::filesystem::remove(oddChunk);

    const auto rf64 = std::filesystem::temp_directory_path() / "aura-wav-rf64-contract.wav";
    {
        std::ofstream file(rf64, std::ios::binary);
        const uint32_t ds64Size = 28;
        const uint64_t riffSize = 8 + ds64Size + 8 + 16 + 8 + 4;
        const uint64_t dataSize = 4;
        const uint64_t sampleCount = 2;
        const uint32_t tableLength = 0;
        const uint16_t pcm = 1, channels = 1, bits = 16, align = 2;
        const uint32_t sampleRate = 48000, byteRate = 96000;
        const int16_t samples[2] = {16384, -16384};
        file.write("RF64", 4); const uint32_t unknown = 0xFFFFFFFFu;
        file.write(reinterpret_cast<const char*>(&unknown), 4); file.write("WAVE", 4);
        file.write("ds64", 4); file.write(reinterpret_cast<const char*>(&ds64Size), 4);
        file.write(reinterpret_cast<const char*>(&riffSize), 8);
        file.write(reinterpret_cast<const char*>(&dataSize), 8);
        file.write(reinterpret_cast<const char*>(&sampleCount), 8);
        file.write(reinterpret_cast<const char*>(&tableLength), 4);
        const uint32_t fmtSize = 16;
        file.write("fmt ", 4); file.write(reinterpret_cast<const char*>(&fmtSize), 4);
        file.write(reinterpret_cast<const char*>(&pcm), 2); file.write(reinterpret_cast<const char*>(&channels), 2);
        file.write(reinterpret_cast<const char*>(&sampleRate), 4); file.write(reinterpret_cast<const char*>(&byteRate), 4);
        file.write(reinterpret_cast<const char*>(&align), 2); file.write(reinterpret_cast<const char*>(&bits), 2);
        file.write("data", 4); file.write(reinterpret_cast<const char*>(&unknown), 4);
        file.write(reinterpret_cast<const char*>(samples), sizeof(samples));
    }
    Aura::Core::IO::WavDecoder rf64Decoder;
    assert(rf64Decoder.open(rf64.string()));
    Aura::Core::AudioBuffer rf64Buffer;
    rf64Decoder.decodeFull(rf64Buffer);
    assert(rf64Buffer.getNumChannels() == 1 && rf64Buffer.getNumSamples() == 2);
    assert(std::abs(rf64Buffer.getReadPointer(0)[0] - 0.5f) < 1.0e-5f);
    assert(std::abs(rf64Buffer.getReadPointer(0)[1] + 0.5f) < 1.0e-5f);
    std::filesystem::remove(rf64);

    const auto truncatedRiff = std::filesystem::temp_directory_path() / "aura-wav-truncated-riff-contract.wav";
    {
        std::ofstream file(truncatedRiff, std::ios::binary);
        const uint32_t riffSize = 20;
        const uint32_t oversizedChunk = 32;
        file.write("RIFF", 4);
        file.write(reinterpret_cast<const char*>(&riffSize), 4);
        file.write("WAVE", 4);
        file.write("JUNK", 4);
        file.write(reinterpret_cast<const char*>(&oversizedChunk), 4);
    }
    Aura::Core::IO::WavDecoder truncatedDecoder;
    assert(!truncatedDecoder.open(truncatedRiff.string()));
    std::filesystem::remove(truncatedRiff);

    const auto corrupt = std::filesystem::temp_directory_path() / "aura-wav-corrupt.wav";
    {
        std::ofstream file(corrupt, std::ios::binary);
        file.write("RIFF", 4);
        uint32_t riffSize = 36 + 6;
        file.write(reinterpret_cast<const char*>(&riffSize), 4);
        file.write("WAVEfmt ", 8);
        uint32_t fmtSize = 16;
        file.write(reinterpret_cast<const char*>(&fmtSize), 4);
        uint16_t pcm = 1, channels = 2, bits = 16, align = 1;
        uint32_t sampleRate = 48000, byteRate = 1;
        file.write(reinterpret_cast<const char*>(&pcm), 2);
        file.write(reinterpret_cast<const char*>(&channels), 2);
        file.write(reinterpret_cast<const char*>(&sampleRate), 4);
        file.write(reinterpret_cast<const char*>(&byteRate), 4);
        file.write(reinterpret_cast<const char*>(&align), 2);
        file.write(reinterpret_cast<const char*>(&bits), 2);
    }
    bool rejected = false;
    try {
        Aura::IO::WavLoader::WavInfo corruptInfo{};
        (void)Aura::IO::WavLoader::load(corrupt.string(), corruptInfo);
    } catch (const std::runtime_error&) {
        rejected = true;
    }
    assert(rejected);
    std::filesystem::remove(corrupt);
    return 0;
}
