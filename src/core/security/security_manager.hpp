#pragma once
#include <atomic>
#include <vector>
#include <mutex>
#include <cstdint>
#include <cstring>

namespace Aura::Core::Security {

/**
 * @class SecurityManager
 * @brief Manages memory integrity and process-level security.
 * HONEST FIX: Purged 'Sovereign Shield' branding and prepared for real checksumming.
 */
class SecurityManager {
public:
    static SecurityManager& getInstance() {
        static SecurityManager instance;
        return instance;
    }

    /**
     * @brief Enables active memory monitoring.
     */
    void enableMonitoring() {
        m_monitoringEnabled.store(true);
    }

    /**
     * @brief Performs an integrity check on registered memory regions.
     */
    bool verifyIntegrity() {
        if (!m_monitoringEnabled.load()) return true;
        
        std::lock_guard<std::mutex> lock(m_mutex);
        for (const auto& region : m_protectedRegions) {
            if (region.ptr == nullptr || region.size == 0 ||
                calculateCRC32(static_cast<const uint8_t*>(region.ptr), region.size) != region.checksum) {
                return false;
            }
        }
        return true;
    }

    /**
     * @brief Registers a memory block for monitoring.
     */
    void registerRegion(void* ptr, size_t size) {
        if (ptr == nullptr || size == 0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_protectedRegions.push_back({ptr, size,
            calculateCRC32(static_cast<const uint8_t*>(ptr), size)});
    }

private:
    static uint32_t calculateCRC32(const uint8_t* data, size_t size) noexcept {
        uint32_t crc = 0xFFFFFFFFu;
        for (size_t i = 0; i < size; ++i) {
            crc ^= data[i];
            for (unsigned bit = 0; bit < 8; ++bit) {
                crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
            }
        }
        return ~crc;
    }

    struct Region { void* ptr; size_t size; uint32_t checksum; };
    std::atomic<bool> m_monitoringEnabled{false};
    std::vector<Region> m_protectedRegions;
    std::mutex m_mutex;

    SecurityManager() = default;
};

} // namespace Aura::Core::Security
