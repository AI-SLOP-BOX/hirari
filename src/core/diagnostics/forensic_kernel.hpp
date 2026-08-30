#pragma once
#include <string>
#include <mutex>
#include "forensic_journaler.hpp"

namespace Aura::Core::Diagnostics {

class ForensicKernel {
public:
    static ForensicKernel& getInstance() {
        static ForensicKernel instance;
        return instance;
    }

    void recordDecision(int phaseId, const std::string& description, float value) {
        std::lock_guard<std::mutex> lock(m_mutex);
        
        // FNV-1a 32-bit hash for real-time safe string compression
        uint32_t hash = 2166136261u;
        for (char c : description) {
            hash ^= static_cast<uint8_t>(c);
            hash *= 16777619u;
        }
        
        // Log this through ForensicJournaler for unified diagnostics
        ForensicJournaler::getInstance().log(static_cast<uint32_t>(phaseId), hash, value, 0);
    }

private:
    ForensicKernel() = default;
    std::mutex m_mutex;
};

} // namespace Aura::Core::Diagnostics
