#pragma once
#include <vector>
#include <atomic>
#include <cstdint>
#include <map>
#include <mutex>
#include <string>
#include <vector>

namespace Aura::Core::Network {

/**
 * @class CRDTEngine
 * @brief Multi-user state synchronization using LWW (Last-Write-Wins) register CRDT.
 */
class CRDTEngine {
public:
    struct LWWRegister {
        float value;
        uint64_t timestamp;
        uint32_t userId;

        void merge(const LWWRegister& other) {
            if (other.timestamp > timestamp || (other.timestamp == timestamp && other.userId > userId)) {
                value = other.value;
                timestamp = other.timestamp;
                userId = other.userId;
            }
        }
    };

    struct Update {
        std::string key;
        LWWRegister value{};
    };

    void apply(const Update& update) {
        if (update.key.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto [it, inserted] = m_registers.emplace(update.key, update.value);
        if (!inserted) it->second.merge(update.value);
    }

    void applyBatch(const std::vector<Update>& updates) {
        std::lock_guard<std::mutex> lock(m_mutex);
        for (const auto& update : updates) {
            if (update.key.empty()) continue;
            auto [it, inserted] = m_registers.emplace(update.key, update.value);
            if (!inserted) it->second.merge(update.value);
        }
    }

    bool read(const std::string& key, LWWRegister& out) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_registers.find(key);
        if (it == m_registers.end()) return false;
        out = it->second;
        return true;
    }

    std::vector<Update> snapshot() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<Update> result;
        result.reserve(m_registers.size());
        for (const auto& [key, value] : m_registers) result.push_back({key, value});
        return result;
    }

private:
    mutable std::mutex m_mutex;
    std::map<std::string, LWWRegister> m_registers;
};

} // namespace Aura::Core::Network
