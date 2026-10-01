// WAV writer/loader round-trip contract.
//
// Regression coverage for an earlier false alarm: a report that short exports
// "decoded as 2 frames" turned out to be a harness bug (the literal 2 was
// passed as the frame count instead of the buffer length). This file pins the
// behaviour that is actually correct, so a future mistake in either direction
// - a writer that truncates or a loader that under-reports - is caught.
//
// Verified against the RIFF bytes on disk, not just the loader: data chunk
// size must equal frames * channels * 4 bytes for IEEE float output.
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <vector>

#include "src/io/persistence/wav_writer.hpp"
#include "src/io/wav_loader_utils.hpp"

namespace {

namespace fs = std::filesystem;

std::vector<uint8_t> readAll(const fs::path& path) {
    std::ifstream file(path, std::ios::binary);
    return std::vector<uint8_t>((std::istreambuf_iterator<char>(file)),
                                std::istreambuf_iterator<char>());
}

uint32_t readLe32(const std::vector<uint8_t>& bytes, size_t offset) {
    return static_cast<uint32_t>(bytes[offset]) |
           (static_cast<uint32_t>(bytes[offset + 1]) << 8) |
           (static_cast<uint32_t>(bytes[offset + 2]) << 16) |
           (static_cast<uint32_t>(bytes[offset + 3]) << 24);
}

uint16_t readLe16(const std::vector<uint8_t>& bytes, size_t offset) {
    return static_cast<uint16_t>(bytes[offset] | (bytes[offset + 1] << 8));
}

// Locate the data chunk without assuming it is the second chunk; the Rust
// writer emits a padding JUNK chunk before fmt.
bool findDataChunk(const std::vector<uint8_t>& bytes, uint32_t& size, uint16_t& blockAlign,
                   uint16_t& channels, uint32_t& sampleRate) {
    if (bytes.size() < 44) return false;
    if (std::string(reinterpret_cast<const char*>(bytes.data()), 4) != "RIFF") return false;
    if (std::string(reinterpret_cast<const char*>(bytes.data() + 8), 4) != "WAVE") return false;
    size_t offset = 12;
    while (offset + 8 <= bytes.size()) {
        const std::string tag(reinterpret_cast<const char*>(bytes.data() + offset), 4);
        const uint32_t chunkSize = readLe32(bytes, offset + 4);
        if (tag == "fmt ") {
            channels = readLe16(bytes, offset + 10);
            sampleRate = readLe32(bytes, offset + 12);
            blockAlign = readLe16(bytes, offset + 20);
        } else if (tag == "data") {
            size = chunkSize;
            return true;
        }
        offset += 8 + chunkSize + (chunkSize & 1);
    }
    return false;
}

// Export `frames` of stereo float and require an exact round-trip through both
// the loader and the raw RIFF bytes.
void checkStereoRoundTrip(size_t frames, uint32_t sampleRate) {
    std::vector<float> left(frames);
    std::vector<float> right(frames);
    for (size_t i = 0; i < frames; ++i) {
        left[i] = static_cast<float>(i) * 0.25f - 1.0f;
        right[i] = -(static_cast<float>(i) * 0.25f - 1.0f);
    }
    const fs::path path = fs::temp_directory_path() / "hirari-wav-roundtrip-contract.wav";
    std::error_code ignored;
    fs::remove(path, ignored);

    assert(Hirari::IO::Persistence::WavWriter::write(
        path.string(), left.data(), right.data(), static_cast<uint64_t>(frames), sampleRate));

    Hirari::IO::WavLoader::WavInfo info{};
    const auto decoded = Hirari::IO::WavLoader::load(path.string(), info);
    assert(decoded.size() == 2);
    assert(info.sampleRate == sampleRate);
    assert(info.numChannels == 2);
    // The whole point: the decoded length must equal the frames written.
    assert(decoded[0].size() == frames);
    assert(decoded[1].size() == frames);
    for (size_t i = 0; i < frames; ++i) {
        assert(decoded[0][i] == left[i]);
        assert(decoded[1][i] == right[i]);
    }

    // Independently confirm the container, so a writer that agreed with its own
    // loader but wrote a malformed header would still be caught.
    const auto bytes = readAll(path);
    uint32_t dataSize = 0;
    uint16_t blockAlign = 0;
    uint16_t channels = 0;
    uint32_t rate = 0;
    assert(findDataChunk(bytes, dataSize, blockAlign, channels, rate));
    assert(channels == 2);
    assert(rate == sampleRate);
    assert(blockAlign == 8);  // 2 channels * 4 bytes (IEEE float)
    assert(dataSize == frames * 2 * 4);

    fs::remove(path, ignored);
}

// Multichannel interleaved PCM16 path, including the shortest possible buffer.
void checkMultichannelPcm16() {
    const std::vector<std::vector<float>> channels{{0.25f}, {-0.25f}, {0.5f}, {-0.5f}};
    const fs::path path = fs::temp_directory_path() / "hirari-wav-pcm16-contract.wav";
    std::error_code ignored;
    fs::remove(path, ignored);

    assert(Hirari::IO::Persistence::WavWriter::writePcm16Interleaved(path.string(), channels, 44'100));

    Hirari::IO::WavLoader::WavInfo info{};
    const auto decoded = Hirari::IO::WavLoader::load(path.string(), info);
    assert(decoded.size() == channels.size());
    assert(info.numChannels == channels.size());
    assert(info.sampleRate == 44'100);
    for (size_t channel = 0; channel < channels.size(); ++channel) {
        assert(decoded[channel].size() == channels[channel].size());
    }
    fs::remove(path, ignored);
}

}  // namespace

int main() {
    // Short buffers are where a frame-count bug would hide.
    checkStereoRoundTrip(1, 48'000);
    checkStereoRoundTrip(2, 48'000);
    checkStereoRoundTrip(4, 48'000);
    checkStereoRoundTrip(6, 48'000);
    checkStereoRoundTrip(64, 44'100);
    checkStereoRoundTrip(512, 48'000);
    checkMultichannelPcm16();
    std::printf("WAV round-trip contract passed\n");
    return 0;
}