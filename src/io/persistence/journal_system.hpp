#pragma once

#include <vector>
#include <string>
#include <fstream>
#include <chrono>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <fcntl.h>
#include <cstring>
#include <thread>
#include <mutex>
#include <atomic>
#include <cerrno>
#include <condition_variable>

namespace Aura::IO::Persistence {

/**
 * @class JournalSystem
 * @brief Differential Layered Persistence (USD / Pixar style) with remote collaboration sync.
 * Saves action deltas to binary logs and broadcasts them over UDP to collaborators asynchronously.
 */
class JournalSystem {
public:
    static JournalSystem& getInstance() { static JournalSystem i; return i; }

    ~JournalSystem() {
        m_running.store(false, std::memory_order_relaxed);
        m_queueWake.notify_one();
        if (m_workerThread.joinable()) {
            m_workerThread.join();
        }
        if (m_socketFd >= 0) {
            close(m_socketFd);
        }
    }

    struct ActionDelta {
        uint64_t timestamp;
        uint32_t trackId;
        uint32_t paramId;
        float value;
    };

    /**
     * @brief Append action to queue in a thread-safe manner (no blocking file I/O).
     */
    void logAction(uint32_t trackId, uint32_t paramId, float value) {
        ActionDelta delta = { 
            static_cast<uint64_t>(std::chrono::system_clock::now().time_since_epoch().count()),
            trackId, paramId, value 
        };
        
        std::lock_guard<std::mutex> lock(m_mutex);
        m_queue.push_back(delta);
        m_queueWake.notify_one();
    }

    /**
     * @brief Registers remote IP address for UDP synchronization.
     */
    void syncRemote(const std::string& peerIp) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_syncPeerIp = peerIp;
        sockaddr_in address{};
        address.sin_family = AF_INET;
        m_peerValid = !peerIp.empty() && inet_pton(AF_INET, peerIp.c_str(), &address.sin_addr) == 1;
        if (m_peerValid && m_socketFd < 0) {
            m_socketFd = socket(AF_INET, SOCK_DGRAM, 0);
            if (m_socketFd >= 0) {
                int flags = fcntl(m_socketFd, F_GETFL, 0);
                if (flags < 0 || fcntl(m_socketFd, F_SETFL, flags | O_NONBLOCK) < 0) {
                    close(m_socketFd);
                    m_socketFd = -1;
                    m_socketFailures.fetch_add(1, std::memory_order_relaxed);
                }
            }
        }
    }

private:
    JournalSystem()
        : m_journalPath("AuraSession.journal")
        , m_socketFd(-1)
        , m_running(true)
    {
        // Start background worker thread to execute I/O and UDP network broadcasts safely
        m_workerThread = std::thread([this]() {
            while (m_running.load(std::memory_order_relaxed) || hasQueuedActions()) {
                std::vector<ActionDelta> localQueue;
                {
                    std::unique_lock<std::mutex> lock(m_mutex);
                    m_queueWake.wait(lock, [this]() {
                        return !m_running.load(std::memory_order_relaxed) || !m_queue.empty();
                    });
                    if (!m_queue.empty()) {
                        localQueue = std::move(m_queue);
                        m_queue.clear();
                    }
                }

                if (!localQueue.empty()) {
                    // 1. Write to journal file using explicit portable CSV text serialization
                    std::ofstream log(m_journalPath, std::ios::app);
                    if (log.is_open()) {
                        for (const auto& delta : localQueue) {
                            log << delta.timestamp << ","
                                << delta.trackId << ","
                                << delta.paramId << ","
                                << delta.value << "\n";
                        }
                        log.close();
                    }

                    // 2. Broadcast portable string payload over non-blocking UDP socket
                    std::lock_guard<std::mutex> lock(m_mutex);
                    if (m_peerValid && m_socketFd >= 0) {
                        sockaddr_in peerAddr;
                        std::memset(&peerAddr, 0, sizeof(peerAddr));
                        peerAddr.sin_family = AF_INET;
                        peerAddr.sin_port = htons(9001);
                        inet_pton(AF_INET, m_syncPeerIp.c_str(), &peerAddr.sin_addr);

                        for (const auto& delta : localQueue) {
                            std::string payload = std::to_string(delta.timestamp) + "," +
                                                  std::to_string(delta.trackId) + "," +
                                                  std::to_string(delta.paramId) + "," +
                                                  std::to_string(delta.value);
                            const ssize_t sent = sendto(m_socketFd, payload.data(), payload.size(), 0,
                                   reinterpret_cast<const sockaddr*>(&peerAddr), sizeof(peerAddr));
                            if (sent < 0 && errno != EINTR) {
                                m_socketFailures.fetch_add(1, std::memory_order_relaxed);
                                m_lastSocketError.store(errno, std::memory_order_relaxed);
                            }
                        }
                    }
                }
            }
        });
    }

    bool hasQueuedActions() {
        std::lock_guard<std::mutex> lock(m_mutex);
        return !m_queue.empty();
    }

    std::vector<ActionDelta> m_queue;
    std::string m_syncPeerIp;
    std::string m_journalPath;
    int m_socketFd;

    std::thread m_workerThread;
    std::condition_variable m_queueWake;
    std::atomic<bool> m_running;
    std::mutex m_mutex;
    bool m_peerValid = false;
    std::atomic<uint64_t> m_socketFailures{0};
    std::atomic<int> m_lastSocketError{0};
};

} // namespace Aura::IO::Persistence
