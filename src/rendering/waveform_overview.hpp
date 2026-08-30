#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <cmath>
#include <mutex>
#include <thread>
#include <atomic>
#include <limits>
#include <exception>
#include <string>
#include <condition_variable>
#include "../core/audio_region.hpp"

namespace Aura::Rendering {

/**
 * @class WaveformOverview
 * @brief Professional multi-resolution peak cache.
 * Fixed: Actually stores and returns peak data for GPU acceleration.
 */
class WaveformOverview {
public:
    struct Peak { float min = 0.0f; float max = 0.0f; };
    struct LOD { uint32_t ratio; std::vector<float> minData; std::vector<float> maxData; };

    WaveformOverview(std::shared_ptr<::Aura::Core::IAudioSource> source) 
        : m_source(source) {
        if (m_source) {
            generateAsync({64, 512, 4096}); 
        }
    }

    ~WaveformOverview() {
        m_stop.store(true, std::memory_order_release);
        if (m_worker.joinable()) m_worker.join();
    }

    WaveformOverview(const WaveformOverview&) = delete;
    WaveformOverview& operator=(const WaveformOverview&) = delete;

    // Pointer access cannot be made safe across a lock release because the
    // worker may publish another vector immediately afterwards. Keep these
    // legacy symbols inert and require copyLOD()/copyBestLOD() for rendering.
    [[deprecated("Use copyLOD or copyBestLOD")]]
    const float* getMinData() const noexcept { return nullptr; }
    [[deprecated("Use copyLOD or copyBestLOD")]]
    const float* getMaxData() const noexcept { return nullptr; }
    size_t getNumPeaks() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_lods.empty() ? 0 : m_lods.front().minData.size();
    }

    bool copyLOD(uint32_t ratio, LOD& destination) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = std::find_if(m_lods.begin(), m_lods.end(),
                                     [ratio](const LOD& lod) { return lod.ratio == ratio; });
        if (it == m_lods.end()) return false;
        destination = *it;
        return true;
    }

    bool copyLODs(std::vector<LOD>& destination) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        destination = m_lods;
        return !destination.empty();
    }

    bool copyBestLOD(uint32_t pixels, LOD& destination) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_lods.empty()) return false;
        const LOD* best = &m_lods.front();
        uint64_t bestDistance = std::numeric_limits<uint64_t>::max();
        for (const auto& lod : m_lods) {
            const uint64_t points = lod.minData.size();
            const uint64_t distance = points > pixels ? points - pixels : pixels - points;
            if (distance < bestDistance) {
                bestDistance = distance;
                best = &lod;
            }
        }
        destination = *best;
        return true;
    }

    bool failed() const noexcept { return m_failed.load(std::memory_order_acquire); }
    bool waitUntilReady(std::chrono::milliseconds timeout) const {
        std::unique_lock<std::mutex> lock(m_readyMutex);
        return m_ready.wait_for(lock, timeout, [this] {
            std::lock_guard<std::mutex> cacheLock(m_mutex);
            return m_failed.load(std::memory_order_acquire) ||
                   m_lods.size() >= 3 || m_stop.load(std::memory_order_acquire);
        }) && !m_failed.load(std::memory_order_acquire);
    }
    std::string lastError() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_error;
    }

private:
    void generateAsync(std::vector<uint32_t> ratios) {
        m_worker = std::thread([this, ratios = std::move(ratios)]() {
          try {
            for (auto ratio : ratios) {
                if (m_stop.load(std::memory_order_acquire)) return;
                LOD lodLevel; lodLevel.ratio = ratio;
                uint64_t totalS = m_source->getNumSamples();
                const uint64_t pointCount = (totalS + ratio - 1) / ratio;
                if (pointCount > std::numeric_limits<uint32_t>::max()) continue;
                uint32_t numPoints = static_cast<uint32_t>(pointCount);
                if (numPoints == 0) continue;
                lodLevel.minData.resize(numPoints);
                lodLevel.maxData.resize(numPoints);
                
                for (uint32_t i = 0; i < numPoints; ++i) {
                    float minV = std::numeric_limits<float>::infinity();
                    float maxV = -std::numeric_limits<float>::infinity();
                    for (uint32_t s = 0; s < ratio && i * ratio + s < totalS; ++s) {
                        float v = m_source->getSample(0, i * ratio + s);
                        if (!std::isfinite(v)) continue;
                        minV = std::min(minV, v); maxV = std::max(maxV, v);
                    }
                    if (!std::isfinite(minV)) minV = 0.0f;
                    if (!std::isfinite(maxV)) maxV = 0.0f;
                    lodLevel.minData[i] = minV;
                    lodLevel.maxData[i] = maxV;
                }
                
                if (m_stop.load(std::memory_order_acquire)) return;
                std::lock_guard<std::mutex> lock(m_mutex);
                m_lods.push_back(std::move(lodLevel));
                m_ready.notify_all();
            }
          } catch (const std::exception& error) {
              std::lock_guard<std::mutex> lock(m_mutex);
              m_error = error.what();
              m_failed.store(true, std::memory_order_release);
              m_ready.notify_all();
          } catch (...) {
              std::lock_guard<std::mutex> lock(m_mutex);
              m_error = "waveform overview generation failed";
              m_failed.store(true, std::memory_order_release);
              m_ready.notify_all();
          }
        });
    }

private:
    std::shared_ptr<::Aura::Core::IAudioSource> m_source;
    std::vector<LOD> m_lods;
    mutable std::mutex m_mutex;
    std::atomic<bool> m_stop{false};
    std::atomic<bool> m_failed{false};
    std::string m_error;
    std::thread m_worker;
    mutable std::condition_variable m_ready;
    mutable std::mutex m_readyMutex;
};

} // namespace Aura::Rendering
