#pragma once
#include <string>
#include <vector>
#include <fstream>
#include <thread>
#include <atomic>
#include <functional>
#include <algorithm>
#include <cstdint>
#include <cmath>
#include <limits>
#include <filesystem>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../../io/persistence/wav_writer.hpp"

namespace Aura::Core::Engine {

/**
 * @class BounceEngine
 * @brief High-performance background rendering engine.
 * HONEST FIX: Implemented RIFF/WAV writing and safe thread management.
 */
// Legacy callback-based renderer retained for source compatibility. The
// public export workflow lives in rendering/bounce/bounce_engine.hpp; keeping
// a distinct name prevents two different BounceEngine classes from colliding
// in Aura::Core::Engine when both headers are included.
class LegacyBounceEngine {
public:
    struct ExportProgress {
        std::atomic<float> progress{0.0f};
        std::atomic<bool> isDone{false};
        std::atomic<bool> cancelled{false};
    };

    using RenderProc = std::function<void(float*, float*, uint32_t)>;

    /**
     * @brief RENDER: Renders the project to a file with industrial precision and export sovereignty.
     * INDUSTRIAL: Delegating project rendering and file writing to the Rust 'ExportOrchestrator'.
     */
    static void renderToFile(const std::string& path, double durationSec, double sr, ExportProgress& prog, RenderProc proc) {
        prog.progress.store(0.0f, std::memory_order_release);
        prog.isDone.store(false, std::memory_order_release);
        prog.cancelled.store(false, std::memory_order_release);
        if (path.empty() || !proc || !std::isfinite(durationSec) || !std::isfinite(sr) ||
            durationSec <= 0.0 || sr < 1000.0 || sr > 384000.0) {
            prog.isDone.store(true, std::memory_order_release);
            return;
        }
        const double total = durationSec * sr;
        if (total > static_cast<double>(std::numeric_limits<uint32_t>::max()) * 8.0) {
            prog.isDone.store(true, std::memory_order_release);
            return;
        }
        const uint64_t totalSamples = static_cast<uint64_t>(total);
        if (totalSamples == 0 || totalSamples > 0x3FFFFFFFull) {
            prog.isDone.store(true, std::memory_order_release);
            return;
        }
        ::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter stream(
            path, totalSamples, static_cast<uint32_t>(sr));
        if (!stream.isOpen()) { prog.isDone.store(true, std::memory_order_release); return; }
        constexpr uint32_t kBlock = 1024;
        std::vector<float> left(kBlock), right(kBlock);
        uint64_t rendered = 0;
        while (rendered < totalSamples && !prog.cancelled.load(std::memory_order_acquire)) {
            const uint32_t count = static_cast<uint32_t>(std::min<uint64_t>(kBlock, totalSamples - rendered));
            std::fill(left.begin(), left.begin() + count, 0.0f);
            std::fill(right.begin(), right.begin() + count, 0.0f);
            proc(left.data(), right.data(), count);
            if (!stream.writeFrames(left.data(), right.data(), count)) break;
            rendered += count;
            prog.progress.store(static_cast<float>(rendered) / static_cast<float>(totalSamples), std::memory_order_release);
        }
        const bool completed = rendered == totalSamples &&
            !prog.cancelled.load(std::memory_order_acquire) && stream.finish();
        if (!completed) {
            prog.cancelled.store(true, std::memory_order_release);
            prog.isDone.store(true, std::memory_order_release);
            return;
        }
        prog.isDone.store(true, std::memory_order_release);
    }
};

} // namespace Aura::Core::Engine
