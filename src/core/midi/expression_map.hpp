#pragma once
#include <algorithm>
#include <cstdint>
#include <string>
#include <vector>

namespace Aura::Core::MIDI {
struct MpeNote { uint64_t start=0,length=0; uint8_t channel=0,note=0,velocity=0; int16_t pitchCents=0; uint16_t pressure=0,timbre=0; };
struct Articulation { std::string name; uint8_t program=0; int16_t transpose=0; uint8_t channel=0; };
class ExpressionMap {
public:
    bool add(Articulation a){if(a.name.empty())return false; auto it=std::find_if(m_items.begin(),m_items.end(),[&](const auto&x){return x.name==a.name;}); if(it!=m_items.end())*it=std::move(a); else m_items.push_back(std::move(a)); return true;}
    bool remove(const std::string& name){auto it=std::find_if(m_items.begin(),m_items.end(),[&](const auto&x){return x.name==name;});if(it==m_items.end())return false;m_items.erase(it);return true;}
    const Articulation* find(const std::string& name)const{auto it=std::find_if(m_items.begin(),m_items.end(),[&](const auto&x){return x.name==name;});return it==m_items.end()?nullptr:&*it;}
    const std::vector<Articulation>& items()const noexcept{return m_items;}
private: std::vector<Articulation> m_items;
};
inline bool validMpeNote(const MpeNote& n){return n.length>0&&n.channel<16&&n.note<=127&&n.velocity<=127;}
}
