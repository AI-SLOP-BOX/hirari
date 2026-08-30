#pragma once

#include <stdint.h>
#include <string>
#include <vector>
#include <map>
#include <mutex>
#include <unordered_map>
#include <fstream>
#include <filesystem>
#include <chrono>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::Core::Database {

/**
 * @struct DBRecord
 * @brief Sovereign Forensic Record (Industrial).
 */
struct DBRecord {
    uint64_t timestamp;
    std::string category;
    std::string key;
    std::string value;
};

class ProjectDB {
public:
    static constexpr size_t kMaxInMemRecords = 10000;

    static ProjectDB& i() {
        static ProjectDB instance;
        return instance;
    }

    void recordForensic(const std::string& cat, const std::string& key, const std::string& val) {
        std::lock_guard<std::mutex> lock(m_mutex);
        
        DBRecord record{
            static_cast<uint64_t>(std::chrono::system_clock::now().time_since_epoch().count()),
            cat, key, val
        };

        if (m_records.size() >= kMaxInMemRecords) {
            m_records.erase(m_records.begin());
            m_categoryIndex.clear();
            for (size_t i = 0; i < m_records.size(); ++i) {
                m_categoryIndex.emplace(m_records[i].category, i);
            }
        }
        
        m_records.push_back(record);
        m_categoryIndex.emplace(cat, m_records.size() - 1);
    }

    std::vector<DBRecord> queryByCategory(const std::string& cat) {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<DBRecord> result;
        auto range = m_categoryIndex.equal_range(cat);
        for (auto it = range.first; it != range.second; ++it) {
            if (it->second < m_records.size()) {
                result.push_back(m_records[it->second]);
            }
        }
        return result;
    }

    bool flushToDisk(const std::string& path) {
        if (path.empty()) return false;
        std::vector<DBRecord> snapshot;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            snapshot = m_records;
        }
        const std::filesystem::path destination(path);
        static std::atomic<uint64_t> temporarySequence{0};
        const auto nonce = temporarySequence.fetch_add(1, std::memory_order_relaxed);
        const std::filesystem::path temporary = destination.string() + ".tmp-" +
            std::to_string(static_cast<unsigned long long>(
#if defined(_WIN32)
                0
#else
                static_cast<unsigned long long>(::getpid())
#endif
            )) + "-" + std::to_string(static_cast<unsigned long long>(nonce));
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file) return false;
        const uint32_t version = 1;
        const uint32_t count = static_cast<uint32_t>(std::min<size_t>(snapshot.size(), kMaxInMemRecords));
        constexpr char magic[] = {'A', 'U', 'R', 'A', '\x01', 'D', 'B'};
        file.write(magic, sizeof(magic));
        file.write(reinterpret_cast<const char*>(&version), sizeof(version));
        file.write(reinterpret_cast<const char*>(&count), sizeof(count));
        for (uint32_t i = 0; i < count; ++i) {
            const auto& record = snapshot[i];
            const uint32_t catLen = static_cast<uint32_t>(std::min<size_t>(record.category.size(), 4096));
            const uint32_t keyLen = static_cast<uint32_t>(std::min<size_t>(record.key.size(), 4096));
            const uint32_t valueLen = static_cast<uint32_t>(std::min<size_t>(record.value.size(), 1u << 20));
            file.write(reinterpret_cast<const char*>(&record.timestamp), sizeof(record.timestamp));
            file.write(reinterpret_cast<const char*>(&catLen), sizeof(catLen));
            file.write(record.category.data(), catLen);
            file.write(reinterpret_cast<const char*>(&keyLen), sizeof(keyLen));
            file.write(record.key.data(), keyLen);
            file.write(reinterpret_cast<const char*>(&valueLen), sizeof(valueLen));
            file.write(record.value.data(), valueLen);
        }
        file.flush();
        if (!file) {
            file.close();
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
        file.close();
#if !defined(_WIN32)
        const int fileFd = ::open(temporary.c_str(), O_RDONLY);
        if (fileFd < 0 || ::fsync(fileFd) != 0) {
            if (fileFd >= 0) ::close(fileFd);
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
        ::close(fileFd);
#endif
        std::error_code ec;
        std::filesystem::rename(temporary, destination, ec);
        if (ec) {
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
#if !defined(_WIN32)
        const auto parent = destination.parent_path().empty()
            ? std::filesystem::path(".") : destination.parent_path();
        const int dirFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
        if (dirFd < 0 || ::fsync(dirFd) != 0) {
            if (dirFd >= 0) ::close(dirFd);
            return false;
        }
        ::close(dirFd);
#endif
        return true;
    }

private:
    ProjectDB() = default;
    std::vector<DBRecord> m_records;
    std::unordered_multimap<std::string, size_t> m_categoryIndex;
    std::mutex m_mutex;
};

} // namespace Aura::Core::Database
