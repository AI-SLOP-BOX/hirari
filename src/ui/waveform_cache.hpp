#pragma once
#include <vector>
#include <map>
#include <mutex>
#include <thread>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <future>
#include <set>
#include <cstdint>
#include <chrono>
#include <exception>
#include <string>
#include "../core/concurrency/thread_pool.hpp"

namespace Aura::UI {

/**
 * @struct WaveformLevel
 * @brief Downsampled waveform data for a specific resolution.
 */
struct WaveformLevel {
    std::vector<float> minPeaks;
    std::vector<float> maxPeaks;
};

/**
 * @class WaveformCache
 * @brief Asynchronous waveform generation and caching.
 */
class WaveformCache {
public:
    static WaveformCache& getInstance() {
        static WaveformCache instance;
        return instance;
    }

    ~WaveformCache() {
        std::lock_guard<std::mutex> tasksLock(m_tasksMutex);
        for (auto& task : m_tasks) {
            if (task.valid()) task.wait();
        }
    }

    /**
     * @brief Requests waveform data for a region. 
     * If not in cache, spawns a background task.
     */
    const WaveformLevel* getWaveform(uint32_t regionId, uint32_t resolution) {
        // Return a per-thread snapshot rather than a pointer into m_cache.
        // The cache may be replaced by a worker immediately after the mutex is
        // released; exposing the map element would create a dangling pointer.
        thread_local WaveformLevel snapshot;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_cache.find({regionId, resolution});
        if (it != m_cache.end()) {
            snapshot = it->second;
            return &snapshot;
        }
        
        // Generation is explicitly requested through requestWaveform(), which
        // accepts decoded samples and owns the worker lifecycle. This lookup
        // path remains read-only so UI rendering never starts hidden work.
        return nullptr;
    }

    /**
     * @brief Asynchronously builds a min/max overview from interleaved samples.
     * @param resolution Number of output columns/pixels to generate.
     * @return false when the request is invalid, already cached, or already pending.
     *
     * The sample vector is moved into the worker, so callers can release their
     * decode buffer immediately. Results are committed only if the region's
     * generation is still current; clearRegion() therefore cannot be undone by
     * a late worker completion.
     */
    bool requestWaveform(uint32_t regionId, uint32_t resolution,
                         std::vector<float> samples) {
        if (regionId == 0 || resolution == 0 || samples.empty()) return false;

        reapCompleted();
        const Key key{regionId, resolution};
        uint64_t generation = 0;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            if (m_cache.find(key) != m_cache.end() || m_pending.count(key) != 0) {
                return false;
            }
            generation = m_generations[regionId];
            m_pending[key] = generation;
        }

        std::future<void> task;
        try {
            task = Aura::Core::Concurrency::ThreadPool::getInstance().enqueue(
            [this, regionId, resolution, generation,
             samples = std::move(samples), key]() mutable {
              try {
                WaveformLevel level;
                const size_t columns = std::min<size_t>(resolution, samples.size());
                level.minPeaks.assign(columns, 0.0f);
                level.maxPeaks.assign(columns, 0.0f);
                const size_t samplesPerColumn =
                    (samples.size() + columns - 1) / columns;

                for (size_t column = 0; column < columns; ++column) {
                    const size_t begin = column * samplesPerColumn;
                    const size_t end = std::min(samples.size(), begin + samplesPerColumn);
                    float minimum = 0.0f;
                    float maximum = 0.0f;
                    bool sawFinite = false;
                    for (size_t i = begin; i < end; ++i) {
                        const float sample = samples[i];
                        if (!std::isfinite(sample)) continue;
                        if (!sawFinite) {
                            minimum = maximum = sample;
                            sawFinite = true;
                        } else {
                            minimum = std::min(minimum, sample);
                            maximum = std::max(maximum, sample);
                        }
                    }
                    level.minPeaks[column] = sawFinite ? minimum : 0.0f;
                    level.maxPeaks[column] = sawFinite ? maximum : 0.0f;
                }

                {
                    std::lock_guard<std::mutex> lock(m_mutex);
                    const bool current = m_generations[regionId] == generation;
                    auto pending = m_pending.find(key);
                    if (pending != m_pending.end() && pending->second == generation) {
                        m_pending.erase(pending);
                    }
                    if (current) {
                        m_cache[key] = std::move(level);
                        m_failures.erase(key);
                        m_failureReasons.erase(key);
                    }
                }
              } catch (const std::exception& error) {
                std::lock_guard<std::mutex> lock(m_mutex);
                auto pending = m_pending.find(key);
                if (pending != m_pending.end() && pending->second == generation) {
                    m_pending.erase(pending);
                }
                if (m_generations[regionId] == generation) {
                    m_failures.insert(key);
                    m_failureReasons[key] = error.what();
                    m_lastError = error.what();
                }
              } catch (...) {
                std::lock_guard<std::mutex> lock(m_mutex);
                auto pending = m_pending.find(key);
                if (pending != m_pending.end() && pending->second == generation) {
                    m_pending.erase(pending);
                }
                if (m_generations[regionId] == generation) {
                    m_failures.insert(key);
                    m_failureReasons[key] = "waveform generation failed";
                    m_lastError = m_failureReasons[key];
                }
              }
            });
        } catch (...) {
            std::lock_guard<std::mutex> lock(m_mutex);
            auto pending = m_pending.find(key);
            if (pending != m_pending.end() && pending->second == generation)
                m_pending.erase(pending);
            m_failures.insert(key);
            m_failureReasons[key] = "waveform worker unavailable";
            m_lastError = m_failureReasons[key];
            return false;
        }
        {
            std::lock_guard<std::mutex> tasksLock(m_tasksMutex);
            m_tasks.emplace_back(std::move(task));
        }
        return true;
    }

    bool isPending(uint32_t regionId, uint32_t resolution) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_pending.count({regionId, resolution}) != 0;
    }

    bool hasFailure(uint32_t regionId, uint32_t resolution) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_failures.count({regionId, resolution}) != 0;
    }

    std::string failureReason(uint32_t regionId, uint32_t resolution) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_failureReasons.find({regionId, resolution});
        return it == m_failureReasons.end() ? std::string{} : it->second;
    }

    std::string lastError() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_lastError;
    }

    void waitForPending() {
        {
            std::lock_guard<std::mutex> tasksLock(m_tasksMutex);
            for (auto& task : m_tasks) {
                if (task.valid()) task.wait();
            }
        }
        reapCompleted();
    }

    bool getWaveformCopy(uint32_t regionId, uint32_t resolution,
                         WaveformLevel& destination) const {
        return copyWaveform(regionId, resolution, destination);
    }

    void putWaveform(uint32_t regionId, uint32_t resolution, WaveformLevel level) {
        if (regionId == 0 || resolution == 0 || level.minPeaks.size() != level.maxPeaks.size()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        // An externally supplied cache is authoritative for this region.
        // Advance its generation so an older asynchronous worker cannot
        // publish over it after this function returns.
        ++m_generations[regionId];
        for (auto it = m_pending.begin(); it != m_pending.end();) {
            if (it->first.first == regionId) it = m_pending.erase(it); else ++it;
        }
        m_cache[{regionId, resolution}] = std::move(level);
        m_failures.erase({regionId, resolution});
        m_failureReasons.erase({regionId, resolution});
    }

    bool copyWaveform(uint32_t regionId, uint32_t resolution, WaveformLevel& destination) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_cache.find({regionId, resolution});
        if (it == m_cache.end()) return false;
        destination = it->second;
        return true;
    }

    void clearRegion(uint32_t regionId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        ++m_generations[regionId];
        for (auto it = m_cache.begin(); it != m_cache.end();) {
            if (it->first.first == regionId) it = m_cache.erase(it); else ++it;
        }
        for (auto it = m_pending.begin(); it != m_pending.end();) {
            if (it->first.first == regionId) it = m_pending.erase(it); else ++it;
        }
        for (auto it = m_failures.begin(); it != m_failures.end();) {
            if (it->first == regionId) it = m_failures.erase(it); else ++it;
        }
        for (auto it = m_failureReasons.begin(); it != m_failureReasons.end();) {
            if (it->first.first == regionId) it = m_failureReasons.erase(it); else ++it;
        }
    }

private:
    using Key = std::pair<uint32_t, uint32_t>;

    WaveformCache() = default;

    void reapCompleted() {
        std::lock_guard<std::mutex> tasksLock(m_tasksMutex);
        for (auto it = m_tasks.begin(); it != m_tasks.end();) {
            if (!it->valid() || it->wait_for(std::chrono::milliseconds(0)) ==
                                    std::future_status::ready) {
                if (it->valid()) {
                    try { it->get(); } catch (...) { /* worker records failures */ }
                }
                it = m_tasks.erase(it);
            } else {
                ++it;
            }
        }
    }

    mutable std::mutex m_mutex;
    mutable std::mutex m_tasksMutex;
    std::map<Key, WaveformLevel> m_cache;
    std::map<Key, uint64_t> m_pending;
    std::set<Key> m_failures;
    std::map<Key, std::string> m_failureReasons;
    std::map<uint32_t, uint64_t> m_generations;
    std::vector<std::future<void>> m_tasks;
    std::string m_lastError;
};

} // namespace Aura::UI
