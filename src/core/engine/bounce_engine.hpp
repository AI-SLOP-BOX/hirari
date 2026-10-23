#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <functional>
#include <string>

extern "C" uint8_t hirari_legacy_bounce_render(
    double duration_sec, double sample_rate, const uint8_t* path, size_t path_size,
    void* context, bool (*render)(void*, float*, float*, uint32_t),
    bool (*cancelled)(void*), void (*progress)(void*, float));

namespace Hirari::Core::Engine {

class LegacyBounceEngine {
public:
    struct ExportProgress {
        std::atomic<float> progress{0.0f};
        std::atomic<bool> isDone{false};
        std::atomic<bool> cancelled{false};
    };
    using RenderProc = std::function<void(float*, float*, uint32_t)>;

    static void renderToFile(const std::string& path, double durationSec, double sr,
                             ExportProgress& progress, RenderProc render) {
        progress.progress.store(0.0f, std::memory_order_release);
        progress.isDone.store(false, std::memory_order_release);
        progress.cancelled.store(false, std::memory_order_release);
        if (path.empty() || !render) {
            progress.isDone.store(true, std::memory_order_release);
            return;
        }
        Context context{&progress, &render, {}};
        const auto result = hirari_legacy_bounce_render(
            durationSec, sr, reinterpret_cast<const uint8_t*>(path.data()),
            path.size(), &context, &renderBlock, &isCancelled, &setProgress);
        if (context.exception) std::rethrow_exception(context.exception);
        if (result == 2) progress.cancelled.store(true, std::memory_order_release);
        progress.isDone.store(true, std::memory_order_release);
    }

private:
    struct Context { ExportProgress* progress; RenderProc* render; std::exception_ptr exception; };

    static bool renderBlock(void* opaque, float* left, float* right, uint32_t count) {
        auto* context = static_cast<Context*>(opaque);
        try { (*context->render)(left, right, count); return true; }
        catch (...) { context->exception = std::current_exception(); return false; }
    }
    static bool isCancelled(void* opaque) {
        return static_cast<Context*>(opaque)->progress->cancelled.load(std::memory_order_acquire);
    }
    static void setProgress(void* opaque, float value) {
        static_cast<Context*>(opaque)->progress->progress.store(value, std::memory_order_release);
    }
};

} // namespace Hirari::Core::Engine
