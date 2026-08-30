#pragma once

#include <string>
#include <thread>
#include <future>
#include <memory>
#include <atomic>
#include <filesystem>
#include <fstream>
#include <limits>
#include <system_error>
#include <mutex>
#include <vector>
#include "project_encoder.hpp"

#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::IO::Persistence {

struct SaveWorkerHandle {
    std::thread worker;
    std::shared_ptr<std::atomic<bool>> finished;
};

class SaveCompletionGuard final {
public:
    explicit SaveCompletionGuard(std::shared_ptr<std::atomic<bool>> finished)
        : m_finished(std::move(finished)) {}
    ~SaveCompletionGuard() {
        if (m_finished) m_finished->store(true, std::memory_order_release);
    }

private:
    std::shared_ptr<std::atomic<bool>> m_finished;
};

/**
 * @class AsyncSerializer
 * @brief Zero-Stall Background Project Persistence.
 * HONEST FIX: Implements Atomic-Save to prevent data corruption during crashes.
 */
class AsyncSerializer {
public:
    static AsyncSerializer& getInstance() { 
        static AsyncSerializer instance; 
        return instance; 
    }

    /**
     * @brief SERIALIZE: Professional background save using temp files.
     * Starts a background thread to encode and write project data.
     * HONEST FIX: Uses std::shared_ptr to avoid massive string copies on the main thread.
     */
    std::future<bool> serializeAsync(const std::string& path, std::string jsonData) {
        constexpr size_t kMaxProjectBytes = 256u * 1024u * 1024u;
        if (path.empty() || jsonData.empty() || jsonData.size() > kMaxProjectBytes) {
            std::promise<bool> p;
            p.set_value(false);
            return p.get_future();
        }
        const auto savingFlag = m_savingFlag;
        if (savingFlag->exchange(true)) {
            std::promise<bool> p;
            p.set_value(false);
            return p.get_future();
        }

        // Reap only completed workers. Joining an in-flight save here would
        // turn this background API into a synchronous UI stall under load.
        {
            std::lock_guard<std::mutex> workersLock(m_workersMutex);
            std::vector<SaveWorkerHandle> pending;
            pending.reserve(m_workers.size());
            for (auto& worker : m_workers) {
                if (worker.finished->load(std::memory_order_acquire)) {
                    if (worker.worker.joinable()) worker.worker.join();
                } else {
                    pending.push_back(std::move(worker));
                }
            }
            m_workers = std::move(pending);
        }

        auto dataPtr = std::make_shared<std::string>(std::move(jsonData));
        auto result = std::make_shared<std::promise<bool>>();
        auto future = result->get_future();
        static std::atomic<uint64_t> saveSequence{0};
        const auto sequence = saveSequence.fetch_add(1, std::memory_order_relaxed);
#if defined(_WIN32)
        const std::string tmpPath = path + ".tmp-" + std::to_string(sequence);
#else
        const std::string tmpPath = path + ".tmp-" +
            std::to_string(static_cast<unsigned long>(::getpid())) + "-" +
            std::to_string(sequence);
#endif

        // Keep the writer owned by the serializer. The returned future reports
        // completion, while the owned thread guarantees that process shutdown
        // cannot leave a save worker running against destroyed global state.
        try {
        auto finished = std::make_shared<std::atomic<bool>>(false);
        std::lock_guard<std::mutex> workersLock(m_workersMutex);
        m_workers.push_back(SaveWorkerHandle{
            std::thread([path, dataPtr, result, tmpPath, savingFlag, finished]() {
            SaveCompletionGuard completion(finished);
            std::unique_ptr<SaveLock> saveLock;
            try {
                const std::filesystem::path output(path);
                if (output.has_parent_path()) {
                    std::error_code directoryError;
                    std::filesystem::create_directories(output.parent_path(), directoryError);
                    if (directoryError) {
                        savingFlag->store(false, std::memory_order_release);
                        result->set_value(false);
                        return false;
                    }
                }
                saveLock = std::make_unique<SaveLock>(path);
                if (!saveLock->acquired()) {
                    savingFlag->store(false, std::memory_order_release);
                    result->set_value(false);
                    return false;
                }
                // 1. Write to temp file
                std::ofstream file(tmpPath, std::ios::binary | std::ios::trunc);
                if (!file.is_open()) {
                    savingFlag->store(false, std::memory_order_release);
                    saveLock->release();
                    result->set_value(false);
                    return false;
                }
                file.write(dataPtr->data(), static_cast<std::streamsize>(dataPtr->size()));
                file.flush();
                if (!file) {
                    file.close();
                    std::error_code cleanupError;
                    std::filesystem::remove(tmpPath, cleanupError);
                    savingFlag->store(false, std::memory_order_release);
                    saveLock->release();
                    result->set_value(false);
                    return false;
                }
                file.close();
                if (file.fail()) {
                    std::error_code cleanupError;
                    std::filesystem::remove(tmpPath, cleanupError);
                    savingFlag->store(false, std::memory_order_release);
                    saveLock->release();
                    result->set_value(false);
                    return false;
                }
#if !defined(_WIN32)
                const int fileFd = ::open(tmpPath.c_str(), O_RDONLY);
                if (fileFd < 0 || ::fsync(fileFd) != 0) {
                    if (fileFd >= 0) ::close(fileFd);
                    std::error_code cleanupError;
                    std::filesystem::remove(tmpPath, cleanupError);
                    savingFlag->store(false, std::memory_order_release);
                    saveLock->release();
                    result->set_value(false);
                    return false;
                }
                ::close(fileFd);
#endif

                // 2. Atomic Rename (POSIX rename is atomic)
                std::error_code ec;
                std::filesystem::rename(tmpPath, path, ec);
                if (ec) {
                    // Never remove the last known-good project to recover a
                    // failed replacement. The caller can retry safely.
                }
                if (ec) {
                    std::error_code cleanupError;
                    std::filesystem::remove(tmpPath, cleanupError);
                }
                const bool ok = !ec;
#if !defined(_WIN32)
                if (ok) {
                    const std::filesystem::path output(path);
                    const auto parent = output.parent_path().empty() ?
                        std::filesystem::path(".") : output.parent_path();
                    const int dirFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
                    if (dirFd < 0 || ::fsync(dirFd) != 0) {
                        if (dirFd >= 0) ::close(dirFd);
                        savingFlag->store(false, std::memory_order_release);
                        saveLock->release();
                        result->set_value(false);
                        return false;
                    }
                    ::close(dirFd);
                }
#endif
                // Keep the serializer busy until both the file rename and
                // the containing-directory durability barrier complete.
                // Clearing this earlier permits a second save to begin while
                // the first publication is not yet durable.
                savingFlag->store(false, std::memory_order_release);
                saveLock->release();
                result->set_value(ok);
                return ok;
            } catch (...) {
                std::error_code ec;
                std::filesystem::remove(tmpPath, ec);
                savingFlag->store(false, std::memory_order_release);
                if (saveLock) saveLock->release();
                result->set_value(false);
                return false;
            }
            }),
            std::move(finished),
        });
        } catch (...) {
            savingFlag->store(false, std::memory_order_release);
            result->set_value(false);
        }
        return future;
    }

    bool isSaving() const { return m_savingFlag->load(std::memory_order_acquire); }

private:
    class SaveLock {
    public:
        explicit SaveLock(const std::string& target)
            : path(target + ".save.lock") {
#if defined(_WIN32)
            acquiredFlag = true;
#else
            fd = ::open(path.c_str(), O_CREAT | O_EXCL | O_WRONLY, 0600);
            acquiredFlag = fd >= 0;
#endif
        }

        ~SaveLock() {
            release();
        }

        void release() noexcept {
            if (!acquiredFlag) return;
#if !defined(_WIN32)
            if (fd >= 0) ::close(fd);
            fd = -1;
#endif
            std::error_code ignored;
            std::filesystem::remove(path, ignored);
            acquiredFlag = false;
        }

        bool acquired() const noexcept { return acquiredFlag; }

    private:
        std::filesystem::path path;
        bool acquiredFlag = false;
#if !defined(_WIN32)
        int fd = -1;
#endif
    };

    AsyncSerializer() : m_savingFlag(std::make_shared<std::atomic<bool>>(false)) {}
    ~AsyncSerializer() {
        std::lock_guard<std::mutex> workersLock(m_workersMutex);
        for (auto& worker : m_workers) {
            if (worker.worker.joinable()) worker.worker.join();
        }
    }
    std::shared_ptr<std::atomic<bool>> m_savingFlag;
    std::mutex m_workersMutex;
    std::vector<SaveWorkerHandle> m_workers;
};

} // namespace Aura::IO::Persistence
