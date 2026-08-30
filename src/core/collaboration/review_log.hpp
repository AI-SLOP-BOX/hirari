#pragma once
#include <algorithm>
#include <cstdint>
#include <string>
#include <vector>
#include <fstream>

namespace Aura::Core::Collaboration {
struct ReviewNote { uint64_t id=0, sample=0; std::string author, text; bool resolved=false; };
struct ChangeRecord { uint64_t revision=0; std::string author, action; };
class ReviewLog {
public:
    uint64_t addNote(ReviewNote n){if(n.author.empty()||n.text.empty())return 0;n.id=++m_next;n_notes.push_back(std::move(n));return m_next;}
    bool resolve(uint64_t id,bool value=true){auto it=std::find_if(n_notes.begin(),n_notes.end(),[&](const auto&n){return n.id==id;});if(it==n_notes.end())return false;it->resolved=value;return true;}
    bool remove(uint64_t id){auto it=std::find_if(n_notes.begin(),n_notes.end(),[&](const auto&n){return n.id==id;});if(it==n_notes.end())return false;n_notes.erase(it);return true;}
    const std::vector<ReviewNote>& notes()const noexcept{return n_notes;}
    void recordChange(ChangeRecord c){if(!c.author.empty()&&!c.action.empty())m_changes.push_back(std::move(c));}
    const std::vector<ChangeRecord>& changes()const noexcept{return m_changes;}
    bool save(const std::string& path)const{std::ofstream f(path);if(!f)return false;for(const auto&n:n_notes)f<<"N\t"<<n.id<<'\t'<<n.sample<<'\t'<<n.author<<'\t'<<n.resolved<<'\t'<<n.text<<'\n';for(const auto&c:m_changes)f<<"C\t"<<c.revision<<'\t'<<c.author<<'\t'<<c.action<<'\n';return static_cast<bool>(f);}
private: uint64_t m_next=0; std::vector<ReviewNote>n_notes; std::vector<ChangeRecord>m_changes;
};
}
