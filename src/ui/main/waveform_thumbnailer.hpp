#pragma once
#include <vector>
#include <map>
#include <shared_mutex>
#include <algorithm>
#include <string>
#include <memory>
#include <cmath>
#include <limits>
#include "../../core/concurrency/audio_task_manager.hpp"

#if defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#elif defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace Aura::UI::Main {

/**
 * @class WaveformThumbnailer
 * @brief High-performance SIMD-accelerated waveform peak generator.
 * HONEST FIX: Replaced std::async with managed task pool and implemented real SIMD.
 */
class WaveformThumbnailer {
public:
    static constexpr size_t kMaxInputSamples = 16u * 1024u * 1024u;
    // Bound copied-but-not-yet-processed audio across all queued requests.
    // Per-request validation alone still permits many large assets to be
    // queued at once and exhaust the UI process.
    static constexpr size_t kMaxQueuedInputSamples = 64u * 1024u * 1024u;
    static WaveformThumbnailer& getInstance() {
        static WaveformThumbnailer instance;
        return instance;
    }

    struct PeakData {
        std::vector<float> peakPairs; 
    };

    /**
     * @brief Generates peaks asynchronously using the managed thread pool.
     */
    void generateAsync(const std::string& assetPath, const float* data, size_t numSamples, size_t resolution = 128) {
        if (assetPath.empty() || !data || numSamples == 0 || numSamples > kMaxInputSamples || resolution == 0) return;
        size_t queued = m_queuedInputSamples.load(std::memory_order_relaxed);
        for (;;) {
            if (queued > kMaxQueuedInputSamples ||
                numSamples > kMaxQueuedInputSamples - queued) return;
            if (m_queuedInputSamples.compare_exchange_weak(
                    queued, queued + numSamples, std::memory_order_acq_rel,
                    std::memory_order_relaxed)) break;
        }
        uint64_t generation = 0;
        {
            std::unique_lock lock(m_cacheMutex);
            generation = ++m_generations[assetPath];
        }
        GenerationTask* taskData = nullptr;
        try {
            taskData = new GenerationTask{ this, assetPath, {}, numSamples, resolution, generation, numSamples };
            taskData->samples.assign(data, data + numSamples);
        } catch (...) {
            delete taskData;
            m_queuedInputSamples.fetch_sub(numSamples, std::memory_order_acq_rel);
            return;
        }

        ::Aura::Core::Concurrency::AudioTaskStealingScheduler::getInstance().postTask(0, {
            [](void* d) {
                auto* td = static_cast<GenerationTask*>(d);
                try {
                    td->owner->processTask(td);
                } catch (...) {
                    // A malformed/oversized asset must not terminate the UI
                    // worker. QueueBudgetGuard in processTask releases the
                    // reserved input budget during unwinding.
                }
                delete td;
            },
            taskData
        });
    }

    // Invalidate work for an asset without requiring a new audio buffer. Any
    // task already queued will finish harmlessly but cannot publish its old
    // result because its generation is stale.
    void invalidate(const std::string& assetPath) {
        if (assetPath.empty()) return;
        std::unique_lock lock(m_cacheMutex);
        ++m_generations[assetPath];
        m_cache.erase(assetPath);
    }

    // Returns an immutable ownership token. The caller may retain it after
    // this method returns without racing a later cache publication.
    std::shared_ptr<const PeakData> getPeaks(const std::string& assetPath) const {
        std::shared_lock lock(m_cacheMutex);
        auto it = m_cache.find(assetPath);
        return (it != m_cache.end()) ? it->second : nullptr;
    }

private:
#if defined(__x86_64__) || defined(_M_X64)
    static bool avx2Available() noexcept {
#if defined(__GNUC__) || defined(__clang__)
        return __builtin_cpu_supports("avx2") != 0;
#else
        return false;
#endif
    }
#endif

    struct GenerationTask {
        WaveformThumbnailer* owner;
        std::string assetPath;
        std::vector<float> samples;
        size_t numSamples;
        size_t resolution;
        uint64_t generation;
        size_t reservedSamples;
    };

    void processTask(GenerationTask* td) {
        struct QueueBudgetGuard {
            WaveformThumbnailer* owner;
            size_t samples;
            ~QueueBudgetGuard() {
                owner->m_queuedInputSamples.fetch_sub(samples, std::memory_order_acq_rel);
            }
        } budget{this, td->reservedSamples};
        PeakData peaks;
        peaks.peakPairs.reserve(((td->numSamples + td->resolution - 1) / td->resolution) * 2);

        for (size_t i = 0; i < td->numSamples; i += td->resolution) {
            float mn = 0.0f, mx = 0.0f;
            size_t end = std::min(i + td->resolution, td->numSamples);
            const size_t count = end - i;
#if defined(__x86_64__) || defined(_M_X64)
            size_t j = i;
            if (avx2Available() && count >= 8) {
                __m256 minV = _mm256_set1_ps(std::numeric_limits<float>::infinity());
                __m256 maxV = _mm256_set1_ps(-std::numeric_limits<float>::infinity());
                for (; j + 8 <= end; j += 8) {
                    const __m256 values = _mm256_loadu_ps(td->samples.data() + j);
                    minV = _mm256_min_ps(minV, values);
                    maxV = _mm256_max_ps(maxV, values);
                }
                alignas(32) float mins[8], maxs[8];
                _mm256_store_ps(mins, minV); _mm256_store_ps(maxs, maxV);
                mn = mins[0]; mx = maxs[0];
                for (size_t lane = 1; lane < 8; ++lane) { mn = std::min(mn, mins[lane]); mx = std::max(mx, maxs[lane]); }
            }
#elif defined(__arm64__) || defined(__aarch64__)
            size_t j = i;
            if (count >= 4) {
                float32x4_t minV = vdupq_n_f32(std::numeric_limits<float>::infinity());
                float32x4_t maxV = vdupq_n_f32(-std::numeric_limits<float>::infinity());
                for (; j + 4 <= end; j += 4) {
                    const float32x4_t values = vld1q_f32(td->samples.data() + j);
                    minV = vminq_f32(minV, values); maxV = vmaxq_f32(maxV, values);
                }
                float mins[4], maxs[4]; vst1q_f32(mins, minV); vst1q_f32(maxs, maxV);
                mn = mins[0]; mx = maxs[0];
                for (size_t lane = 1; lane < 4; ++lane) { mn = std::min(mn, mins[lane]); mx = std::max(mx, maxs[lane]); }
            }
#else
            size_t j = i;
#endif
            if (count > 0 && !(std::isfinite(mn) && std::isfinite(mx))) { mn = 0.0f; mx = 0.0f; }
            for (; j < end; ++j) {
                const float val = std::isfinite(td->samples[j]) ? td->samples[j] : 0.0f;
                mn = (j == i && count < 8) ? val : std::min(mn, val);
                mx = (j == i && count < 8) ? val : std::max(mx, val);
            }

            peaks.peakPairs.push_back(mn);
            peaks.peakPairs.push_back(mx);
        }

        auto published = std::make_shared<const PeakData>(std::move(peaks));
        std::unique_lock lock(m_cacheMutex);
        const auto generation = m_generations.find(td->assetPath);
        if (generation != m_generations.end() && generation->second == td->generation) {
            m_cache[td->assetPath] = std::move(published);
        }
    }

    WaveformThumbnailer() = default;
    
    std::map<std::string, std::shared_ptr<const PeakData>> m_cache;
    std::map<std::string, uint64_t> m_generations;
    std::atomic<size_t> m_queuedInputSamples{0};
    mutable std::shared_mutex m_cacheMutex;
};

} // namespace Aura::UI::Main
