#include <array>
#include <atomic>
#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>
#include <thread>

#if defined(_WIN32)
#include <process.h>
#define AURA_GETPID _getpid
#else
#include <unistd.h>
#define AURA_GETPID getpid
#endif

#include "../src/core/recording_engine.hpp"
#include "../src/io/persistence/auto_save_engine.hpp"

int main() {
    const auto root = std::filesystem::temp_directory_path() /
        ("aura-recording-autosave-" +
         std::to_string(static_cast<unsigned long>(AURA_GETPID())));
    std::error_code ec;
    std::filesystem::remove_all(root, ec);
    assert(std::filesystem::create_directories(root, ec));

    const auto recordingPath = root / "take.wav";
    const auto projectPath = root / "project.json";
    auto& autosave = Aura::IO::Persistence::AutoSaveEngine::getInstance();
    std::atomic<uint64_t> projectGeneration{0};
    autosave.start(projectPath.string(), 1, [&projectGeneration] {
        const uint64_t generation = projectGeneration.load(std::memory_order_acquire);
        // The provider models a project snapshot assembled while recording is
        // active. It must always publish one complete generation, never a
        // partially-written JSON document.
        return std::string("{\"schema\":1,\"generation\":") +
               std::to_string(generation) + "}";
    });

    Aura::Core::RecordingEngine recorder;
    assert(recorder.start(recordingPath.string(), 48'000.0));
    std::array<float, 256> left{};
    std::array<float, 256> right{};
    left.fill(0.2f);
    right.fill(-0.2f);

    for (unsigned block = 1; block <= 256; ++block) {
        assert(recorder.write(left.data(), right.data(),
                              static_cast<uint32_t>(left.size())));
        projectGeneration.store(block, std::memory_order_release);
        autosave.markModified();
        if ((block % 8u) == 0u) {
            assert(autosave.flushNow());
        }
    }

    recorder.stop();
    assert(!recorder.hasWriteError());
    assert(!recorder.hasBufferOverflowed());
    assert(recorder.droppedFrames() == 0);
    projectGeneration.fetch_add(1, std::memory_order_acq_rel);
    autosave.markModified();
    assert(autosave.flushNow());
    autosave.stop();

    assert(std::filesystem::is_regular_file(recordingPath));
    assert(std::filesystem::file_size(recordingPath) ==
           80u + 256u * 256u * 2u * sizeof(float));
    std::ifstream saved(projectPath.string() + ".autosave", std::ios::binary);
    const std::string content((std::istreambuf_iterator<char>(saved)), {});
    assert(content.rfind("{\"schema\":1,\"generation\":", 0) == 0);
    assert(!content.empty() && content.back() == '}');

    for (const auto& entry : std::filesystem::directory_iterator(root)) {
        const auto name = entry.path().filename().string();
        assert(name.find(".tmp-") == std::string::npos);
        assert(name.find(".save.lock") == std::string::npos);
    }

    std::filesystem::remove_all(root, ec);
    assert(!ec);
    return 0;
}
