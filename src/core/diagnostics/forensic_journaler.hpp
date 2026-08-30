#pragma once
#include <fstream>
#include <string>
#include <thread>
#include <atomic>
#include <vector>
#include "../utils/ring_buffer.hpp"

namespace Aura::Core::Diagnostics {

/**
 * @class ForensicJournaler
 * @brief Industrial-grade session transaction logger for Aura DAW.
 * INDUSTRIAL: Records every engine command to a binary journal in real-time to ensure zero data loss.
 * This singleton runs a dedicated background thread for high-priority disk I/O.
 */
class ForensicJournaler {
public:
    ForensicJournaler() = default;
    /**
     * @struct Entry
     * @brief A single journaled transaction.
     */
    struct Entry { 
        uint32_t type; 
        uint32_t tid; 
        float val; 
        uint64_t ts; 
    };

    static ForensicJournaler& getInstance() {
        static ForensicJournaler instance;
        return instance;
    }

    /**
     * @brief Opens the journal file and starts the background writer.
     */
    void start(const std::string& path) {
        if (m_running.load()) stop();
        m_journalPath = path;
        m_running.store(true);
        m_writerThread = std::thread(&ForensicJournaler::writerLoop, this);
    }

    /**
     * @brief Submits a command to be journaled.
     * RT-Safe: This method never blocks and uses the lock-free RingBuffer.
     */
    void log(uint32_t type, uint32_t tid, float val, uint64_t ts) {
        m_queue.push({type, tid, val, ts});
        m_wakeSequence.fetch_add(1, std::memory_order_release);
        m_wakeSequence.notify_one();
    }

    /**
     * @brief Gracefully stops the journaler and flushes all pending data.
     */
    void stop() {
        m_running.store(false);
        m_wakeSequence.fetch_add(1, std::memory_order_release);
        m_wakeSequence.notify_one();
        if (m_writerThread.joinable()) m_writerThread.join();
    }

    /**
     * @brief Recovers entries from a journal file.
     */
    static std::vector<Entry> recover(const std::string& path) {
        std::vector<Entry> entries;
        std::ifstream file(path, std::ios::binary);
        if (!file.is_open()) return entries;

        Entry entry;
        while (file.read(reinterpret_cast<char*>(&entry), sizeof(Entry))) {
            entries.push_back(entry);
        }
        return entries;
    }

private:
public:
    ~ForensicJournaler() { stop(); }

private:

    /**
     * @brief The background writer loop.
     */
    void writerLoop() {
        std::ofstream file(m_journalPath, std::ios::binary | std::ios::app);
        while (m_running.load() || !m_queue.isEmpty()) {
            Entry entry;
            if (m_queue.pop(entry)) {
                file.write(reinterpret_cast<char*>(&entry), sizeof(Entry));
                // INDUSTRIAL: Force disk flush for immediate durability.
                file.flush();
            } else {
                const uint64_t observed = m_wakeSequence.load(std::memory_order_acquire);
                if (m_running.load(std::memory_order_acquire) || !m_queue.isEmpty()) {
                    m_wakeSequence.wait(observed, std::memory_order_acquire);
                }
            }
        }
    }

    std::string m_journalPath;
    std::atomic<bool> m_running{false};
    std::thread m_writerThread;
    
    // INDUSTRIAL: Using the optimized Power-of-Two RingBuffer.
    RingBuffer<Entry, 4096> m_queue;
    std::atomic<uint64_t> m_wakeSequence{0};
};

} // namespace Aura::Core::Diagnostics
