#pragma once

#include <string>
#include <map>
#include <future>
#include <thread>
#include <vector>
#include <atomic>
#include <mutex>
#include <fstream>
#include <filesystem>
#include <limits>
#include <unordered_map>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../../core/utils/string_hash.hpp"

namespace Aura::IO::Assets {

using namespace ::Aura::Core::Utils;

/**
 * @brief CachingSubsystem: High-performance, non-blocking disk caching for track Freezing.
 * Eliminates std::string operations in the audio thread by pre-calculating paths on the IO thread.
 */
class CachingSubsystem {
public:
    static CachingSubsystem& getInstance() {
        static CachingSubsystem instance;
        return instance;
    }

    /**
     * @brief Pre-registers a track for caching to avoid runtime path calculations.
     */
    void registerCacheSlot(uint32_t trackId, const std::string& path) {
        std::lock_guard<std::mutex> lock(m_registryWriteMutex);
        const auto current = std::atomic_load_explicit(&m_pathRegistry, std::memory_order_acquire);
        auto next = std::make_shared<Registry>(current ? *current : Registry{});
        (*next)[trackId] = std::make_shared<const std::string>(path);
        std::atomic_store_explicit(&m_pathRegistry,
                                   std::shared_ptr<const Registry>(std::move(next)),
                                   std::memory_order_release);
    }

    /**
     * @brief Writes an audio block to the cache asynchronously (Off-loaded to IO thread).
     */
    void writeAsync(uint32_t trackId, const std::vector<float>& data) {
        const auto registry = std::atomic_load_explicit(&m_pathRegistry, std::memory_order_acquire);
        if (!registry) return;
        const auto it = registry->find(trackId);
        if (it == registry->end() || !it->second || it->second->empty()) return;
        const std::string path = *it->second;
        const uint64_t generation = nextGeneration(path);
        // Own every worker so shutdown can join it before the singleton and
        // its filesystem state disappear.  The generation gate prevents a
        // slower older block from publishing over a newer cache block.
        std::lock_guard<std::mutex> workerLock(m_workerMutex);
        m_workers.emplace_back([this, path, data, generation]() {
            performDiskWrite(path, data, generation);
        });
    }

    /**
     * @brief Returns the cached path for a track. Real-time safe (No allocations).
     */
    std::shared_ptr<const std::string> getCachedPath(uint32_t trackId) const {
        const auto registry = std::atomic_load_explicit(&m_pathRegistry, std::memory_order_acquire);
        if (!registry) return {};
        const auto it = registry->find(trackId);
        return it == registry->end() ? std::shared_ptr<const std::string>{} : it->second;
    }

private:
    CachingSubsystem() = default;
    ~CachingSubsystem() {
        std::lock_guard<std::mutex> lock(m_workerMutex);
        for (auto& worker : m_workers) {
            if (worker.joinable()) worker.join();
        }
    }

    uint64_t nextGeneration(const std::string& path) {
        std::lock_guard<std::mutex> lock(m_generationMutex);
        return ++m_pathGenerations[path];
    }

    bool isCurrentGeneration(const std::string& path, uint64_t generation) const {
        std::lock_guard<std::mutex> lock(m_generationMutex);
        const auto it = m_pathGenerations.find(path);
        return it != m_pathGenerations.end() && it->second == generation;
    }

    void performDiskWrite(const std::string& path, const std::vector<float>& data,
                          uint64_t generation) {
        if (data.empty() || data.size() > std::numeric_limits<size_t>::max() / sizeof(float)) return;
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
        const std::filesystem::path temporary = output.string() + ".tmp-aura-cache-" + std::to_string(id);
        std::error_code ec;
        if (output.has_parent_path()) {
            std::filesystem::create_directories(output.parent_path(), ec);
            if (ec) return;
        }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) return;
        file.write(reinterpret_cast<const char*>(data.data()),
                   static_cast<std::streamsize>(data.size() * sizeof(float)));
        file.flush();
        const bool written = static_cast<bool>(file);
        file.close();
        if (!written) {
            std::filesystem::remove(temporary, ec);
            return;
        }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::filesystem::remove(temporary, ec);
            return;
        }
        ::close(fd);
#endif
        // Keep the generation check and publication atomic with respect to
        // newer writers. A check followed by an unlocked rename lets an old
        // worker overwrite a newer cache if the next generation is issued in
        // between those two operations.
        {
            std::lock_guard<std::mutex> generationLock(m_generationMutex);
            const auto current = m_pathGenerations.find(path);
            if (current == m_pathGenerations.end() || current->second != generation) {
                std::filesystem::remove(temporary, ec);
                return;
            }
            std::filesystem::rename(temporary, output, ec);
        }
        if (ec) std::filesystem::remove(temporary, ec);
#if !defined(_WIN32)
        if (!ec && output.has_parent_path()) {
            const int directoryFd = ::open(output.parent_path().c_str(), O_RDONLY);
            if (directoryFd >= 0) {
                (void)::fsync(directoryFd);
                ::close(directoryFd);
            }
        }
#endif
    }

    // Professional cache pre-registration: Avoids std::string concatenation at runtime.
    using Registry = std::map<uint32_t, std::shared_ptr<const std::string>>;
    mutable std::mutex m_registryWriteMutex;
    std::shared_ptr<const Registry> m_pathRegistry = std::make_shared<const Registry>();
    mutable std::mutex m_generationMutex;
    std::unordered_map<std::string, uint64_t> m_pathGenerations;
    mutable std::mutex m_workerMutex;
    std::vector<std::thread> m_workers;
};

} // namespace Aura::IO::Assets
