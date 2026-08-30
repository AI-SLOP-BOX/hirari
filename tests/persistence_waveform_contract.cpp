#include <cassert>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <atomic>
#include <thread>
#include <vector>

#include "../src/graphics/gui_asset_accelerator.hpp"
#include "../src/io/persistence/auto_save_engine.hpp"

int main() {
    auto& accelerator = Aura::Graphics::GUIAssetAccelerator::getInstance();
    accelerator.generateWaveformCache("contract", {0.0f, 1.0f, -0.5f, 0.25f}, 48000);
    const auto cache = accelerator.getCache("contract");
    assert(cache.sampleRate == 48000);
    assert(cache.levels.size() == 3);
    assert(cache.minPeaks.size() == 4 && cache.maxPeaks.size() == 4);
    assert(cache.minPeaks[1] == 1.0f && cache.maxPeaks[2] == -0.5f);

    Aura::Graphics::WaveformCache copy;
    assert(accelerator.copyCache("contract", copy));
    assert(copy.maxPeaks == cache.maxPeaks);
    std::vector<float> mins, maxs;
    assert(accelerator.copyLevelForWidth("contract", 2, mins, maxs));
    assert(mins.size() == maxs.size() && !mins.empty());
    assert(accelerator.getCache("missing").levels.empty());

    const auto project = std::filesystem::temp_directory_path() / "aura-autosave-contract.json";
    std::error_code ec;
    std::filesystem::remove(project.string() + ".autosave", ec);
    auto& autosave = Aura::IO::Persistence::AutoSaveEngine::getInstance();
    autosave.start(project.string(), 1, [] { return std::string("{\"tracks\":1}"); });
    autosave.markModified();
    const bool saved = autosave.flushNow();
    autosave.stop();
    assert(saved);
    std::ifstream file(project.string() + ".autosave");
    const std::string content((std::istreambuf_iterator<char>(file)), {});
    assert(content == "{\"tracks\":1}");
    std::filesystem::remove(project.string() + ".autosave", ec);

    // Concurrent manual-style flushes must serialize through the save mutex
    // and leave one complete snapshot, never a partially published JSON file.
    const auto concurrentProject = std::filesystem::temp_directory_path() /
        "aura-autosave-concurrent-contract.json";
    std::filesystem::remove(concurrentProject.string() + ".autosave", ec);
    std::filesystem::remove(concurrentProject.string() + ".autosave.save.lock", ec);
    std::atomic<unsigned> snapshotVersion{0};
    autosave.start(concurrentProject.string(), 60, [&snapshotVersion] {
        const unsigned version = snapshotVersion.fetch_add(1, std::memory_order_relaxed) + 1;
        return std::string("{\"schema\":1,\"generation\":") +
               std::to_string(version) + "}";
    });
    autosave.markModified();
    std::vector<std::thread> flushers;
    for (unsigned i = 0; i < 8; ++i) {
        flushers.emplace_back([&autosave] { (void)autosave.flushNow(); });
    }
    for (auto& flusher : flushers) flusher.join();
    autosave.stop();

    std::ifstream concurrentFile(concurrentProject.string() + ".autosave");
    const std::string concurrentContent(
        (std::istreambuf_iterator<char>(concurrentFile)), {});
    assert(concurrentContent.rfind("{\"schema\":1,\"generation\":", 0) == 0);
    assert(concurrentContent.back() == '}');
    assert(!std::filesystem::exists(concurrentProject.string() + ".autosave.save.lock"));
    for (const auto& entry : std::filesystem::directory_iterator(concurrentProject.parent_path())) {
        const auto name = entry.path().filename().string();
        assert(name.find("aura-autosave-concurrent-contract.json.autosave.tmp-") == std::string::npos);
    }
    std::filesystem::remove(concurrentProject.string() + ".autosave", ec);
    return 0;
}
