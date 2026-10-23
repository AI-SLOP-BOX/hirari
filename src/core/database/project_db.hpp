#pragma once

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

#include "core/rust_ffi.hpp"

namespace Hirari::Core::Database {

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

    ProjectDB(const ProjectDB&) = delete;
    ProjectDB& operator=(const ProjectDB&) = delete;

    ~ProjectDB() { hirari_project_db_destroy(state_); }

    void recordForensic(const std::string& category, const std::string& key,
                        const std::string& value) {
        const auto timestamp = static_cast<uint64_t>(
            std::chrono::system_clock::now().time_since_epoch().count());
        hirari_project_db_record(state_, timestamp,
            reinterpret_cast<const uint8_t*>(category.data()), category.size(),
            reinterpret_cast<const uint8_t*>(key.data()), key.size(),
            reinterpret_cast<const uint8_t*>(value.data()), value.size());
    }

    std::vector<DBRecord> queryByCategory(const std::string& category) {
        std::vector<DBRecord> result;
        hirari_project_db_query(state_,
            reinterpret_cast<const uint8_t*>(category.data()), category.size(),
            &result, [](void* context, uint64_t timestamp,
                        const uint8_t* recordCategory, size_t categorySize,
                        const uint8_t* key, size_t keySize,
                        const uint8_t* value, size_t valueSize) {
                auto& records = *static_cast<std::vector<DBRecord>*>(context);
                records.push_back({timestamp,
                    std::string(reinterpret_cast<const char*>(recordCategory), categorySize),
                    std::string(reinterpret_cast<const char*>(key), keySize),
                    std::string(reinterpret_cast<const char*>(value), valueSize)});
            });
        return result;
    }

    bool flushToDisk(const std::string& path) {
        return hirari_project_db_flush(state_,
            reinterpret_cast<const uint8_t*>(path.data()), path.size());
    }

private:
    ProjectDB() : state_(hirari_project_db_create()) {}
    void* state_;
};

} // namespace Hirari::Core::Database
