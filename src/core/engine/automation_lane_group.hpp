#pragma once
#include <string>
#include <unordered_map>
#include <vector>

namespace Aura::Core::Engine {
struct AutomationLaneState { uint32_t id=0; bool linked=false, protectedLane=false, preview=false; float previewValue=0; };
class AutomationLaneGroup {
public:
    bool add(uint32_t id){if(!id)return false;m_lanes.emplace(id,AutomationLaneState{id});return true;}
    bool remove(uint32_t id){return m_lanes.erase(id)!=0;}
    bool link(uint32_t id,bool value){auto it=m_lanes.find(id);if(it==m_lanes.end())return false;it->second.linked=value;return true;}
    bool protect(uint32_t id,bool value){auto it=m_lanes.find(id);if(it==m_lanes.end())return false;it->second.protectedLane=value;return true;}
    bool preview(uint32_t id,float value){auto it=m_lanes.find(id);if(it==m_lanes.end()||it->second.protectedLane)return false;it->second.preview=true;it->second.previewValue=value;return true;}
    const AutomationLaneState* get(uint32_t id)const{auto it=m_lanes.find(id);return it==m_lanes.end()?nullptr:&it->second;}
private:std::unordered_map<uint32_t,AutomationLaneState>m_lanes;
};
}
