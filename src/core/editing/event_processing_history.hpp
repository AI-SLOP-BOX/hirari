#pragma once
#include <cstdint>
#include <cstddef>
#include <string>
#include <vector>
#include <algorithm>
#include <cmath>

namespace aura::editing {
struct EventProcessingStep {
    uint32_t id = 0;
    std::string operation;
    float parameter = 0.0f;
    bool enabled = true;
};

class EventProcessingHistory {
public:
    bool add(const std::string& operation, float parameter) {
        if (operation.empty() || operation.size() > 128 || !std::isfinite(parameter) || m_steps.size() >= 4096) return false;
        m_steps.push_back(EventProcessingStep{m_next_id++, operation, parameter, true}); return true;
    }
    bool setEnabled(uint32_t id, bool enabled) { for (auto& s:m_steps) if(s.id==id){s.enabled=enabled;return true;} return false; }
    bool remove(uint32_t id) { auto n=m_steps.size(); m_steps.erase(std::remove_if(m_steps.begin(),m_steps.end(),[&](const auto& s){return s.id==id;}),m_steps.end()); return n!=m_steps.size(); }
    bool move(uint32_t id, size_t targetIndex) { auto it=std::find_if(m_steps.begin(),m_steps.end(),[&](const auto& s){return s.id==id;}); if(it==m_steps.end()||targetIndex>=m_steps.size()) return false; auto value=*it; m_steps.erase(it); m_steps.insert(m_steps.begin()+std::min(targetIndex,m_steps.size()),value); return true; }
    std::vector<EventProcessingStep> activeSteps() const { std::vector<EventProcessingStep> out; for(const auto& s:m_steps) if(s.enabled) out.push_back(s); return out; }
    const std::vector<EventProcessingStep>& steps() const { return m_steps; }
private: std::vector<EventProcessingStep> m_steps; uint32_t m_next_id=1;
};
}
