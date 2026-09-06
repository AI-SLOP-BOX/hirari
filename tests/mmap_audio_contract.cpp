#include "../src/io/mmap_audio_file.hpp"

#include <cassert>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <stdexcept>

namespace {
void writeWav(const std::filesystem::path& path, uint32_t sampleRate,
              uint16_t channels = 1) {
    std::ofstream file(path, std::ios::binary);
    const uint32_t dataSize = 2;
    const uint32_t riffSize = 36 + dataSize;
    const uint16_t format = 1, bits = 16, blockAlign = channels * 2;
    const uint32_t byteRate = sampleRate * blockAlign;
    const int16_t sample = 16384;
    file.write("RIFF", 4);
    file.write(reinterpret_cast<const char*>(&riffSize), 4);
    file.write("WAVEfmt ", 8);
    const uint32_t fmtSize = 16;
    file.write(reinterpret_cast<const char*>(&fmtSize), 4);
    file.write(reinterpret_cast<const char*>(&format), 2);
    file.write(reinterpret_cast<const char*>(&channels), 2);
    file.write(reinterpret_cast<const char*>(&sampleRate), 4);
    file.write(reinterpret_cast<const char*>(&byteRate), 4);
    file.write(reinterpret_cast<const char*>(&blockAlign), 2);
    file.write(reinterpret_cast<const char*>(&bits), 2);
    file.write("data", 4);
    file.write(reinterpret_cast<const char*>(&dataSize), 4);
    file.write(reinterpret_cast<const char*>(&sample), 2);
}
}

int main() {
    const auto valid = std::filesystem::temp_directory_path() / "aura-mmap-valid.wav";
    writeWav(valid, 48000);
    Aura::IO::MMapAudioFile mapped(valid.string());
    assert(mapped.isValid());
    assert(mapped.getNumChannels() == 1);
    assert(mapped.getNumSamples() == 1);
    assert(mapped.getSample(0, 0) > 0.49f && mapped.getSample(0, 0) < 0.51f);
    assert(mapped.getSample(1, 0) == 0.0f);
    std::filesystem::remove(valid);

    const auto invalid = std::filesystem::temp_directory_path() / "aura-mmap-invalid-rate.wav";
    writeWav(invalid, 500000);
    bool rejected = false;
    try {
        Aura::IO::MMapAudioFile shouldReject(invalid.string());
        (void)shouldReject;
    } catch (const std::runtime_error&) {
        rejected = true;
    }
    std::filesystem::remove(invalid);
    assert(rejected);
    return 0;
}
