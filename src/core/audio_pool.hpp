#pragma once
#include <vector>
#include <string>
#include <memory>
#include <unordered_map>
#include <shared_mutex>
#include <atomic>
#include <utility>
#include "rust_ffi.hpp"
#include "audio_region.hpp"
#include "mmap_audio_source.hpp"
#include "concurrency/thread_pool.hpp"

namespace Hirari::Core {

/**
 * @struct PeakData
 * @brief Pre-calculated visual summary of an audio file for UI waveform rendering.
 */
struct PeakData {
    struct Level {
        std::vector<float> mins;
        std::vector<float> maxs;
        uint32_t step;
    };
    std::vector<Level> levels; 
    std::atomic<bool> isReady{false}; 
};

/**
 * @class AudioPool
 * @brief Owns mapped audio sources and asynchronously publishes waveform peaks.
 * The bounded worker pool owns the source until Rust finishes scanning it.
 */
class AudioPool {
public:
    static AudioPool& getInstance() { static AudioPool i; return i; }

    /**
     * @brief REGISTRATION: Adds a file to the pool and kicks off peak generation asynchronously.
     */
    std::shared_ptr<IAudioSource> addSource(const std::string& path) {
        {
            // O(1) ハッシュ検索。Readは大量のスレッドが同時にアクセスしてもブロックしません。
            std::shared_lock<std::shared_mutex> readLock(m_rwMutex);
            auto existing = m_sources.find(path);
            if (existing != m_sources.end()) return existing->second;
        }

        std::shared_ptr<MMapAudioSource> mappedSource;
        try {
            mappedSource = std::make_shared<MMapAudioSource>(path);
        } catch (...) {
            // Invalid paths, permissions, and malformed WAVE headers are
            // rejected at the pool boundary; they must not escape into UI or
            // audio control callers as an uncaught exception.
            return nullptr;
        }
        std::shared_ptr<IAudioSource> source = mappedSource;
        auto peakData = std::make_shared<PeakData>();

        {
            // 書き込み（追加）の瞬間だけ排他ロックを取る。秒にも満たないナノ秒の処理。
            std::unique_lock<std::shared_mutex> writeLock(m_rwMutex);
            m_sources[path] = source;
            m_peakCache[path] = peakData;
        }
        
        // Peak generation runs asynchronously; retain the mapping until Rust
        // has completed its scan.
        try {
            Concurrency::ThreadPool::getInstance().enqueue([mappedSource, peakData]() {
                generateHierarchicalPeaks(mappedSource->rustHandle(), peakData);
            });
        } catch (...) {
            std::unique_lock<std::shared_mutex> cleanupLock(m_rwMutex);
            auto sourceIt = m_sources.find(path);
            if (sourceIt != m_sources.end() && sourceIt->second == source) {
                m_sources.erase(sourceIt);
                m_peakCache.erase(path);
            }
            return nullptr;
        }
        
        return source;
    }

    /**
     * @brief PEAK CACHE: Retrieves pre-calculated waveform data for UI.
     * GUI（60fps）が毎フレーム読みに来ても、ReadLockにより負荷は完全に分散されます。
     */
    std::shared_ptr<PeakData> getPeaks(const std::string& path) {
        std::shared_lock<std::shared_mutex> readLock(m_rwMutex);
        auto it = m_peakCache.find(path);
        return (it != m_peakCache.end()) ? it->second : nullptr;
    }

    void purgeUnused() {
        std::unique_lock<std::shared_mutex> lock(m_rwMutex);
        for (auto it = m_sources.begin(); it != m_sources.end();) {
            if (it->second.use_count() == 1) {
                m_peakCache.erase(it->first);
                it = m_sources.erase(it);
            } else {
                ++it;
            }
        }
    }

private:
    static void generateHierarchicalPeaks(
        const void* mappedFile, const std::shared_ptr<PeakData>& peakData) {
        void* generated = hirari_audio_pool_peaks_create(mappedFile);
        if (!generated) {
            peakData->isReady.store(true, std::memory_order_release);
            return;
        }
        const auto destroy = [](void* handle) { hirari_audio_pool_peaks_destroy(handle); };
        std::unique_ptr<void, decltype(destroy)> hierarchy(generated, destroy);

        std::vector<PeakData::Level> generatedLevels;
        const size_t levelCount = hirari_audio_pool_peaks_level_count(generated);
        generatedLevels.reserve(levelCount);
        for (size_t index = 0; index < levelCount; ++index) {
            const size_t peakCount = hirari_audio_pool_peaks_level_len(generated, index);
            const float* mins = hirari_audio_pool_peaks_level_min(generated, index);
            const float* maxs = hirari_audio_pool_peaks_level_max(generated, index);
            if (peakCount > 0 && (!mins || !maxs)) {
                generatedLevels.clear();
                break;
            }
            PeakData::Level level;
            level.step = hirari_audio_pool_peaks_level_step(generated, index);
            if (peakCount > 0) {
                level.mins.assign(mins, mins + peakCount);
                level.maxs.assign(maxs, maxs + peakCount);
            }
            generatedLevels.push_back(std::move(level));
        }

        // Publish the complete hierarchy once; the acquire/release flag keeps
        // readers from observing partially copied level vectors.
        peakData->levels = std::move(generatedLevels);
        peakData->isReady.store(true, std::memory_order_release);
    }

    std::unordered_map<std::string, std::shared_ptr<IAudioSource>> m_sources;
    std::unordered_map<std::string, std::shared_ptr<PeakData>> m_peakCache;
    std::shared_mutex m_rwMutex;
};

} // namespace Hirari::Core
