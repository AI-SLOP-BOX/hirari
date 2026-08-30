#include <array>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <vector>

#include "../src/core/io/ffmpeg_engine.hpp"
#include "../src/core/io/audio_decoder.hpp"
#include "../src/io/persistence/wav_writer.hpp"

int main() {
    if (std::system("command -v ffmpeg >/dev/null 2>&1") != 0) {
        std::cerr << "ffmpeg is required for the codec contract\n";
        return 2;
    }
    const auto root = std::filesystem::temp_directory_path() /
                      "aura ffmpeg contract; hostile path";
    std::error_code ec;
    std::filesystem::create_directories(root, ec);
    if (ec) return 3;
    const auto input = root / "input source;$(not-a-command).wav";
    const auto mp3 = root / "output mp3;$(not-a-command).mp3";
    const auto flac = root / "output flac;$(not-a-command).flac";
    std::vector<float> left(4096, 0.0f);
    std::vector<float> right(4096, 0.0f);
    for (size_t i = 0; i < left.size(); ++i) {
        left[i] = static_cast<float>((i % 32) - 16) / 64.0f;
        right[i] = -left[i];
    }
    if (!Aura::IO::Persistence::WavWriter::writePcm16(
            input.string(), left.data(), right.data(), left.size(), 48'000)) return 4;
    auto& ffmpeg = Aura::Core::IO::FFmpegEngine::getInstance();
    if (!ffmpeg.exportToFormat(input.string(), mp3.string(), "libmp3lame") ||
        !ffmpeg.exportToFormat(input.string(), flac.string(), "flac")) return 5;
    std::array<char, 4> mp3Header{};
    std::array<char, 4> flacHeader{};
    std::ifstream mp3File(mp3, std::ios::binary);
    std::ifstream flacFile(flac, std::ios::binary);
    mp3File.read(mp3Header.data(), static_cast<std::streamsize>(mp3Header.size()));
    flacFile.read(flacHeader.data(), static_cast<std::streamsize>(flacHeader.size()));
    if (!mp3File || !flacFile || std::string(flacHeader.data(), 4) != "fLaC" ||
        !(std::string(mp3Header.data(), 3) == "ID3" ||
          (static_cast<unsigned char>(mp3Header[0]) == 0xff &&
           (static_cast<unsigned char>(mp3Header[1]) & 0xe0u) == 0xe0u))) return 6;

    // The import side must use the same safe argv boundary and canonical WAV
    // reader after external decoding. Verify both formats produce finite,
    // bounded audio rather than merely checking their container signatures.
    auto mp3Audio = Aura::Core::IO::AudioDecoderManager::getInstance().importFile(mp3.string());
    if (!mp3Audio || mp3Audio->getNumChannels() == 0 ||
        mp3Audio->getNumSamples() == 0 || mp3Audio->getNumSamples() > 65'536) return 8;
    for (uint32_t channel = 0; channel < mp3Audio->getNumChannels(); ++channel)
        for (uint32_t frame = 0; frame < mp3Audio->getNumSamples(); ++frame)
            if (!std::isfinite(mp3Audio->getReadPointer(channel)[frame])) return 9;

    auto flacAudio = Aura::Core::IO::AudioDecoderManager::getInstance().importFile(flac.string());
    if (!flacAudio || flacAudio->getNumChannels() == 0 ||
        flacAudio->getNumSamples() == 0 || flacAudio->getNumSamples() > 65'536) return 10;
    for (uint32_t channel = 0; channel < flacAudio->getNumChannels(); ++channel)
        for (uint32_t frame = 0; frame < flacAudio->getNumSamples(); ++frame)
            if (!std::isfinite(flacAudio->getReadPointer(channel)[frame])) return 11;

    std::filesystem::remove_all(root, ec);
    return ec ? 7 : 0;
}
