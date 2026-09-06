#include "../src/core/recording_engine.hpp"

#include <cassert>
#include <cstdlib>
#include <filesystem>
#include <chrono>
#include <fstream>
#include <array>
#include <iostream>
#include <string>
#if defined(_WIN32)
#include <process.h>
#define AURA_GETPID _getpid
#else
#include <unistd.h>
#define AURA_GETPID getpid
#endif

int main() {
    using Aura::Core::RecordingEngine;

    const auto root = std::filesystem::temp_directory_path() /
                      ("aura-recording-stress-" + std::to_string(static_cast<unsigned long>(AURA_GETPID())));
    std::error_code ec;
    std::filesystem::create_directories(root, ec);
    assert(!ec);
    const auto output = root / "take.wav";

    unsigned long rounds = 250;
    if (const char* configured = std::getenv("AURA_RECORDING_STRESS_ROUNDS")) {
        char* end = nullptr;
        const unsigned long parsed = std::strtoul(configured, &end, 10);
        if (end != configured && *end == '\0' && parsed > 0 && parsed <= 100000) rounds = parsed;
    }

    // Deliberately cross the writer's 4096-frame drain batch by default. This
    // keeps the normal native contract sensitive to stop-time tail loss while
    // remaining small enough for local iteration.
    unsigned long blocksPerTake = 17;
    if (const char* configured = std::getenv("AURA_RECORDING_STRESS_BLOCKS_PER_TAKE")) {
        char* end = nullptr;
        const unsigned long parsed = std::strtoul(configured, &end, 10);
        if (end != configured && *end == '\0' && parsed > 0 && parsed <= 1000000) {
            blocksPerTake = parsed;
        }
    }

    double durationSeconds = 0.0;
    if (const char* configured = std::getenv("AURA_RECORDING_STRESS_DURATION_SECONDS")) {
        char* end = nullptr;
        const double parsed = std::strtod(configured, &end);
        if (end != configured && *end == '\0' && std::isfinite(parsed) && parsed >= 0.0 && parsed <= 86400.0) {
            durationSeconds = parsed;
        }
    }

    constexpr uint32_t kBlockFrames = 256;
    std::array<float, kBlockFrames> left{};
    std::array<float, kBlockFrames> right{};
    for (uint32_t frame = 0; frame < kBlockFrames; ++frame) {
        left[frame] = 0.1f + static_cast<float>(frame % 17) * 0.001f;
        right[frame] = -left[frame];
    }
    RecordingEngine recorder;
    assert(!recorder.start(output.string(), 7'999.0));
    assert(!recorder.start(output.string(), 384'001.0));
    const auto deadline = durationSeconds > 0.0
        ? std::chrono::steady_clock::now() + std::chrono::duration<double>(durationSeconds)
        : std::chrono::steady_clock::time_point::max();
    unsigned long completedTakes = 0;
    for (unsigned long round = 0; round < rounds; ++round) {
        assert(recorder.start(output.string(), 48'000.0));
        uint64_t expectedFrames = 0;
        for (unsigned long block = 0; block < blocksPerTake; ++block) {
            assert(recorder.write(left.data(), right.data(), kBlockFrames));
            expectedFrames += kBlockFrames;
        }
        recorder.stop();
        assert(!recorder.isRecording());
        assert(!recorder.hasWriteError());
        assert(!recorder.hasBufferOverflowed());
        assert(recorder.droppedFrames() == 0);
        assert(std::filesystem::is_regular_file(output));

        // Verify the finalized payload, not just that rename succeeded. This
        // catches queued-frame loss during stop and stale data leaking into a
        // subsequent take.
        std::ifstream input(output, std::ios::binary);
        std::array<uint8_t, 80> header{};
        input.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        assert(input);
        const uint32_t dataSize = static_cast<uint32_t>(header[76]) |
            (static_cast<uint32_t>(header[77]) << 8u) |
            (static_cast<uint32_t>(header[78]) << 16u) |
            (static_cast<uint32_t>(header[79]) << 24u);
        if (dataSize != expectedFrames * sizeof(float) * RecordingEngine::kChannels) {
            std::cerr << "recording frame mismatch: expected_bytes="
                      << expectedFrames * sizeof(float) * RecordingEngine::kChannels
                      << " actual_bytes=" << dataSize
                      << " dropped=" << recorder.droppedFrames() << '\n';
            return 1;
        }
        assert(std::filesystem::file_size(output) == 80u + dataSize);

        for (const auto& entry : std::filesystem::directory_iterator(root)) {
            const auto name = entry.path().filename().string();
            assert(name == "take.wav");
        }

        ++completedTakes;
        if (std::chrono::steady_clock::now() >= deadline) break;
    }

    assert(completedTakes > 0);
    if (durationSeconds > 0.0) {
        // A duration mode is an explicit long-run gate. It must actually run
        // for the requested budget rather than silently falling back to one
        // short take because the round limit was too small.
        while (std::chrono::steady_clock::now() < deadline) {
            assert(recorder.start(output.string(), 48'000.0));
            for (unsigned long block = 0; block < blocksPerTake; ++block) {
                assert(recorder.write(left.data(), right.data(), kBlockFrames));
            }
            recorder.stop();
            assert(!recorder.hasWriteError());
            assert(!recorder.hasBufferOverflowed());
            assert(recorder.droppedFrames() == 0);
            ++completedTakes;
        }
    }

    std::filesystem::remove_all(root, ec);
    assert(!ec);
    return 0;
}
