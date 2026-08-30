#pragma once

#include <string>
#include <chrono>
#include <thread>
#include <mutex>
#include <atomic>
#include <cstdint>
#include <iostream>
#include <functional>
#include <condition_variable>
#include <utility>
#include <optional>
#include "async_serializer.hpp"

namespace Aura::IO::Persistence {

/**
 * @class AutoSaveEngine
 * @brief Background "Safety-net" for project data.
 */
class AutoSaveEngine {
public:
    static AutoSaveEngine& getInstance() {
        static AutoSaveEngine instance;
        return instance;
    }

    void start(const std::string& projectPath, uint32_t intervalSeconds = 300,
               std::function<std::string()> snapshotProvider = {}) {
        if (m_isRunning.load()) stop();
        {
            std::lock_guard<std::mutex> lock(m_stateMutex);
            m_projectPath = projectPath;
            m_intervalSeconds = std::max<uint32_t>(1, intervalSeconds);
            m_lastSaveTime = std::chrono::steady_clock::now();
        }
        setSnapshotProvider(std::move(snapshotProvider));
        m_isRunning.store(true);
        m_thread = std::thread(&AutoSaveEngine::loop, this);
    }

    void stop() {
        m_isRunning.store(false);
        m_wakeCondition.notify_all();
        if (m_thread.joinable()) m_thread.join();
    }

    void markModified() {
        m_dirtyGeneration.fetch_add(1, std::memory_order_acq_rel);
        m_isDirty.store(true, std::memory_order_release);
    }

    bool flushNow() {
        if (!m_isRunning.load(std::memory_order_acquire)) return false;
        uint64_t generation = 0;
        const bool saved = performBackup(generation);
        m_lastSaveSucceeded.store(saved, std::memory_order_release);
        if (saved && m_dirtyGeneration.load(std::memory_order_acquire) == generation) {
            m_isDirty.store(false, std::memory_order_release);
            std::lock_guard<std::mutex> lock(m_stateMutex);
            m_lastSaveTime = std::chrono::steady_clock::now();
        }
        return saved;
    }

    bool isDirty() const noexcept { return m_isDirty.load(std::memory_order_acquire); }
    uint64_t modificationGeneration() const noexcept {
        return m_dirtyGeneration.load(std::memory_order_acquire);
    }

    std::string projectPath() const {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_projectPath;
    }

    bool lastSaveSucceeded() const noexcept {
        return m_lastSaveSucceeded.load(std::memory_order_acquire);
    }

    void setSnapshotProvider(std::function<std::string()> provider) {
        std::lock_guard<std::mutex> lock(m_providerMutex);
        m_snapshotProvider = std::move(provider);
        m_wakeCondition.notify_all();
    }

private:
    AutoSaveEngine() = default;
    ~AutoSaveEngine() { stop(); }

    void loop() {
        while (m_isRunning.load()) {
            {
                std::unique_lock<std::mutex> lock(m_stateMutex);
                m_wakeCondition.wait_for(lock, std::chrono::seconds(1),
                                         [this] { return !m_isRunning.load(); });
            }
            if (!m_isRunning.load()) break;
            
            auto now = std::chrono::steady_clock::now();
            uint32_t interval = 1;
            {
                std::lock_guard<std::mutex> lock(m_stateMutex);
                interval = m_intervalSeconds;
            }
            if (m_isDirty.load() && 
                std::chrono::duration_cast<std::chrono::seconds>(now - m_lastSaveTime).count() >= interval) {
                uint64_t savedGeneration = 0;
                const bool saved = performBackup(savedGeneration);
                m_lastSaveSucceeded.store(saved, std::memory_order_release);
                if (saved &&
                    m_dirtyGeneration.load(std::memory_order_acquire) == savedGeneration) {
                    m_isDirty.store(false, std::memory_order_release);
                    std::lock_guard<std::mutex> lock(m_stateMutex);
                    m_lastSaveTime = now;
                }
            }
        }
    }

    bool performBackup(uint64_t& savedGeneration) {
        // Manual flushNow() and the periodic worker share the same target.
        // Serialize the complete snapshot/publish operation so an older
        // completion cannot overwrite a newer autosave generation.
        std::lock_guard<std::mutex> saveLock(m_saveMutex);
        std::string autoSavePath;
        {
            std::lock_guard<std::mutex> lock(m_stateMutex);
            autoSavePath = m_projectPath + ".autosave";
        }
        std::function<std::string()> provider;
        {
            std::lock_guard<std::mutex> lock(m_providerMutex);
            provider = m_snapshotProvider;
        }
        if (!provider) return false;
        savedGeneration = m_dirtyGeneration.load(std::memory_order_acquire);
        std::string realData = provider();
        if (realData.empty()) return false;
        // Wait on the autosave worker, not the audio/UI thread.  Clearing the
        // dirty flag before the atomic rename completes can silently lose the
        // latest edit when the disk is full or the destination is locked.
        auto result = Aura::IO::Persistence::AsyncSerializer::getInstance()
                          .serializeAsync(autoSavePath, std::move(realData));
        return result.valid() && result.get();
    }


    std::atomic<bool> m_isRunning{false};
    std::atomic<bool> m_isDirty{false};
    std::atomic<uint64_t> m_dirtyGeneration{0};
    std::atomic<bool> m_lastSaveSucceeded{false};
    uint32_t m_intervalSeconds = 300;
    std::string m_projectPath;
    std::chrono::steady_clock::time_point m_lastSaveTime;
    std::thread m_thread;
    mutable std::mutex m_stateMutex;
    std::mutex m_saveMutex;
    std::condition_variable m_wakeCondition;
    std::mutex m_providerMutex;
    std::function<std::string()> m_snapshotProvider;
};

} // namespace Aura::IO::Persistence
