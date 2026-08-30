#pragma once
#include <string>
#include <map>
#include <mutex>

namespace Aura::Core::Assets {

/**
 * @class MetadataRegistry
 * @brief High-speed forensic storage for asset metadata.
 */
class MetadataRegistry {
public:
    static MetadataRegistry& getInstance() {
        static MetadataRegistry instance;
        return instance;
    }

    void storeMetadata(const std::string& assetHash, const std::string& metadata) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_db[assetHash] = metadata;
    }

    std::string getMetadata(const std::string& assetHash) {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_db[assetHash];
    }

private:
    MetadataRegistry() = default;
    std::mutex m_mutex;
    std::map<std::string, std::string> m_db; // INDUSTRIAL: Scale to SQLite/RocksDB
};

} // namespace Aura::Core::Assets
