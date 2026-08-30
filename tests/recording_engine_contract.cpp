#include <cassert>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#include "../src/core/recording_engine.hpp"
#include "../src/io/wav_loader_utils.hpp"

namespace {
template <size_t N>
uint32_t readU32(const std::array<uint8_t, N>& header, size_t offset) {
    return static_cast<uint32_t>(header[offset]) |
           (static_cast<uint32_t>(header[offset + 1]) << 8u) |
           (static_cast<uint32_t>(header[offset + 2]) << 16u) |
           (static_cast<uint32_t>(header[offset + 3]) << 24u);
}
}

int main() {
    const auto path = std::filesystem::temp_directory_path() /
                      "aura-recording-session-isolation-contract.wav";
    std::error_code ec;
    std::filesystem::remove(path, ec);

    Aura::Core::RecordingEngine recorder;
    assert(!recorder.start("", 48'000.0));
    assert(recorder.start(path.string(), 48'000.0));
    const float firstLeft[] = {0.1f, 0.2f, 0.3f, 0.4f};
    const float firstRight[] = {-0.1f, -0.2f, -0.3f, -0.4f};
    assert(recorder.write(firstLeft, firstRight, 4));
    recorder.stop();
    assert(!recorder.isRecording());
    assert(!recorder.hasWriteError());

    // A second take must contain only its own frames, even if the first take
    // was stopped while the writer still had queued data.
    assert(recorder.start(path.string(), 48'000.0));
    const float secondLeft[] = {0.9f, 0.8f};
    const float secondRight[] = {-0.9f, -0.8f};
    assert(recorder.write(secondLeft, secondRight, 2));
    recorder.stop();
    assert(!recorder.hasWriteError());

    std::ifstream input(path, std::ios::binary);
    std::array<uint8_t, 80> header{};
    input.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
    assert(input);
    assert(std::string(reinterpret_cast<const char*>(header.data()), 4) == "RIFF");
    assert(std::string(reinterpret_cast<const char*>(header.data() + 8), 4) == "WAVE");
    assert(std::string(reinterpret_cast<const char*>(header.data() + 72), 4) == "data");
    assert(readU32(header, 76) == 2u * 2u * sizeof(float));
    assert(std::filesystem::file_size(path) == 80u + 2u * 2u * sizeof(float));
    Aura::IO::WavLoader::WavInfo info{};
    const auto decoded = Aura::IO::WavLoader::load(path.string(), info);
    assert(info.numChannels == 2 && info.numSamples == 2 && decoded.size() == 2);

    // The writer drains in bounded batches. A stop must still flush every
    // queued batch, not just the first 4096 frames.
    std::vector<float> burstLeft(5000, 0.25f);
    std::vector<float> burstRight(5000, -0.25f);
    assert(recorder.start(path.string(), 48'000.0));
    assert(recorder.write(burstLeft.data(), burstRight.data(),
                          static_cast<uint32_t>(burstLeft.size())));
    recorder.stop();
    assert(!recorder.hasWriteError());
    assert(!recorder.hasBufferOverflowed());
    assert(recorder.droppedFrames() == 0);
    std::ifstream burstInput(path, std::ios::binary);
    std::array<uint8_t, 80> burstHeader{};
    burstInput.read(reinterpret_cast<char*>(burstHeader.data()),
                    static_cast<std::streamsize>(burstHeader.size()));
    assert(burstInput);
    assert(readU32(burstHeader, 76) == 5000u * 2u * sizeof(float));
    assert(std::filesystem::file_size(path) ==
           80u + 5000u * 2u * sizeof(float));

    std::filesystem::remove(path, ec);
    return 0;
}
