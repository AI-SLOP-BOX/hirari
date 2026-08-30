#pragma once
#include <cstdint>
#include <string>
#include <unordered_map>
#include <vector>

namespace Aura::Core::Hardware {
struct MidiPortProfile { std::string id,name; bool input=false,output=false; uint32_t manufacturer=0; };
struct ControlMapping { uint8_t channel=0, controller=0; std::string command; float min=0,max=1; };
class ControllerProfile {
public:
    explicit ControllerProfile(std::string name = {}) : m_name(std::move(name)) {}
    void addPort(MidiPortProfile p){if(!p.id.empty())m_ports[p.id]=std::move(p);}
    const MidiPortProfile* port(const std::string& id)const{auto it=m_ports.find(id);return it==m_ports.end()?nullptr:&it->second;}
    bool learn(ControlMapping m){if(m.command.empty()||m.channel>=16||m.min>m.max)return false;mappings()[key(m.channel,m.controller)]=std::move(m);return true;}
    const ControlMapping* mapping(uint8_t ch,uint8_t cc)const{auto it=m_mappings.find(key(ch&15,cc));return it==m_mappings.end()?nullptr:&it->second;}
    float feedback(uint8_t ch,uint8_t cc,float normalized)const{const auto*m=mapping(ch,cc);if(!m)return 0;return m->min+(m->max-m->min)*(normalized<0?0:normalized>1?1:normalized);}
    const std::string& name()const noexcept{return m_name;}
private:
    static uint16_t key(uint8_t c,uint8_t n){return static_cast<uint16_t>((c<<8)|n);} std::unordered_map<uint16_t,ControlMapping>& mappings(){return m_mappings;}
    std::string m_name; std::unordered_map<std::string,MidiPortProfile>m_ports; std::unordered_map<uint16_t,ControlMapping>m_mappings;
};
}
