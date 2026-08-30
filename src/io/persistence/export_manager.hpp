#pragma once

#include <string>
#include <vector>
#include <future>
#include <iostream>
#include <atomic>
#include <algorithm>
#include <mutex>
#include <thread>
#include "../../rendering/bounce/bounce_engine.hpp"

namespace Aura::Core::IO::Persistence {

/**
 * @class ExportManager
 * @brief Professional Project Export Handler.
 * HONEST FIX: Bridges the UI to the actual BounceEngine rendering logic.
 */
class ExportManager {
public:
    static ExportManager& getInstance() {
        static ExportManager instance;
        return instance;
    }

    enum class Format { WAV, MP3, FLAC };

    /**
     * @brief Initiates an asynchronous "Bounce" of the master output.
     */
    void exportProject(const std::string& path, uint64_t totalSamples, uint32_t sampleRate) {
        exportProject(path, totalSamples, sampleRate, Format::WAV);
    }

    /// Starts an export using the requested delivery codec.  The legacy
    /// overload above remains float-WAV for existing callers.
    void exportProject(const std::string& path, uint64_t totalSamples,
                       uint32_t sampleRate, Format format) {
        std::lock_guard<std::mutex> guard(m_mutex);
        if (m_isExporting.load(std::memory_order_acquire)) return;
        if (m_worker.joinable()) m_worker.join();

        m_progress.store(0.0f);
        m_cancelRequested.store(false, std::memory_order_release);
        m_isExporting.store(true);

        {
            std::lock_guard<std::mutex> errorGuard(m_errorMutex);
            m_lastError.clear();
        }
        m_worker = std::thread([this, path, totalSamples, sampleRate, format]() {
            // Process the bounce using the engine's offline renderer
            Engine::BounceEngine::BounceConfig config;
            config.outputPath = path;
            config.totalSamples = totalSamples;
            config.sampleRate = sampleRate;
            config.format = format == Format::MP3
                ? Engine::BounceEngine::Format::MP3
                : format == Format::FLAC
                    ? Engine::BounceEngine::Format::FLAC
                    : Engine::BounceEngine::Format::WAV_32F;
            config.revealInFinder = false;
            config.runAIMasteringReview = false;
            config.cancellation = &m_cancelRequested;
            auto result = Engine::BounceEngine::renderMaster(
                config, [this](float progress) {
                    m_progress.store(std::clamp(progress, 0.0f, 1.0f),
                                     std::memory_order_release);
                });
            
            if (result.success) {
                std::cout << "[Exporter] Successfully rendered to: " << path << " in " << result.elapsedSeconds << "s" << std::endl;
            } else {
                std::lock_guard<std::mutex> errorGuard(m_errorMutex);
                m_lastError = result.message;
                std::cerr << "[Exporter] Render FAILED: " << result.message << std::endl;
            }
            
            m_progress.store(1.0f);
            m_isExporting.store(false, std::memory_order_release);
        });
    }

    float getProgress() const { return m_progress.load(std::memory_order_acquire); }
    bool isExporting() const { return m_isExporting.load(std::memory_order_acquire); }
    void cancelExport() noexcept {
        m_cancelRequested.store(true, std::memory_order_release);
    }
    std::string getLastError() const {
        std::lock_guard<std::mutex> guard(m_errorMutex);
        return m_lastError;
    }

private:
    ExportManager() = default;
    ~ExportManager() {
        std::lock_guard<std::mutex> guard(m_mutex);
        if (m_worker.joinable()) m_worker.join();
    }
    ExportManager(const ExportManager&) = delete;
    ExportManager& operator=(const ExportManager&) = delete;

    std::atomic<float> m_progress{0.0f};
    std::atomic<bool> m_isExporting{false};
    std::atomic<bool> m_cancelRequested{false};
    mutable std::mutex m_mutex;
    std::thread m_worker;
    mutable std::mutex m_errorMutex;
    std::string m_lastError;
};

} // namespace Aura::Core::IO::Persistence
