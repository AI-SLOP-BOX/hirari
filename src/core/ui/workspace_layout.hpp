#pragma once
#include <algorithm>
#include <string>
#include <unordered_map>
#include <vector>
#include <fstream>

namespace Aura::Core::UI {
struct PanelLayout { std::string id; int x=0,y=0,width=1,height=1; bool visible=true; };
struct KeyBinding { std::string command, key; };
class WorkspaceLayout {
public:
    bool setPanel(PanelLayout p){if(p.id.empty()||p.width<=0||p.height<=0)return false;m_panels[p.id]=std::move(p);return true;}
    bool removePanel(const std::string& id){return m_panels.erase(id)!=0;}
    const PanelLayout* panel(const std::string& id)const{auto it=m_panels.find(id);return it==m_panels.end()?nullptr:&it->second;}
    std::vector<PanelLayout> panels()const{std::vector<PanelLayout>o;for(const auto&[_,p]:m_panels)o.push_back(p);std::sort(o.begin(),o.end(),[](const auto&a,const auto&b){return a.id<b.id;});return o;}
    void bind(KeyBinding b){if(!b.command.empty()&&!b.key.empty())m_bindings[b.command]=std::move(b.key);}
    std::string keyFor(const std::string& command)const{auto it=m_bindings.find(command);return it==m_bindings.end()?std::string{}:it->second;}
    bool setMacro(const std::string& name,std::vector<std::string> commands){if(name.empty()||commands.empty())return false;m_macros[name]=std::move(commands);return true;}
    const std::vector<std::string>* macro(const std::string& name)const{auto it=m_macros.find(name);return it==m_macros.end()?nullptr:&it->second;}
    bool save(const std::string& path) const { std::ofstream f(path); if(!f)return false; for(const auto& p:panels())f<<"P\t"<<p.id<<'\t'<<p.x<<'\t'<<p.y<<'\t'<<p.width<<'\t'<<p.height<<'\t'<<p.visible<<'\n'; for(const auto&[c,k]:m_bindings)f<<"K\t"<<c<<'\t'<<k<<'\n'; for(const auto&[n,cmds]:m_macros){f<<"M\t"<<n;for(const auto&c:cmds)f<<'\t'<<c;f<<'\n';} return static_cast<bool>(f); }
    bool load(const std::string& path) { std::ifstream f(path); if(!f)return false; std::string line; WorkspaceLayout next; while(std::getline(f,line)){std::vector<std::string>x;size_t p=0,q;while((q=line.find('\t',p))!=std::string::npos){x.push_back(line.substr(p,q-p));p=q+1;}x.push_back(line.substr(p));if(x.empty())continue;try{if(x[0]=="P"&&x.size()==7)next.setPanel({x[1],std::stoi(x[2]),std::stoi(x[3]),std::stoi(x[4]),std::stoi(x[5]),x[6]=="1"});else if(x[0]=="K"&&x.size()==3)next.bind({x[1],x[2]});else if(x[0]=="M"&&x.size()>2)next.setMacro(x[1],std::vector<std::string>(x.begin()+2,x.end()));}catch(...){return false;}}*this=std::move(next);return true; }
private: std::unordered_map<std::string,PanelLayout>m_panels;std::unordered_map<std::string,std::string>m_bindings;std::unordered_map<std::string,std::vector<std::string>>m_macros;
};
}
