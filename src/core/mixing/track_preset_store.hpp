#pragma once
#include "channel_configuration.hpp"
#include <string>
#include <unordered_map>
#include <vector>
#include <algorithm>
#include <fstream>
#include <filesystem>
#include <cmath>

namespace Aura::Core::Mixing {
class TrackPresetStore {
public:
    bool save(const std::string& name,const ChannelConfiguration& c){
        if (!valid_name(name) || !valid(c)) return false;
        if (m_presets.find(name) == m_presets.end() && m_presets.size() >= kMaxPresets) return false;
        m_presets[name]=c; return true;
    }
    bool remove(const std::string& name){return m_presets.erase(name)!=0;}
    const ChannelConfiguration* find(const std::string& name)const{auto it=m_presets.find(name);return it==m_presets.end()?nullptr:&it->second;}
    std::vector<std::string> names()const{std::vector<std::string>o;for(const auto&[n,_]:m_presets)o.push_back(n);std::sort(o.begin(),o.end());return o;}
    bool apply(const std::string& name,ChannelConfiguration& target)const{const auto*p=find(name);if(!p)return false;target=*p;return true;}
    bool saveFile(const std::filesystem::path& path) const {
        if (path.empty()) return false;
        const auto temp = path.string() + ".tmp";
        std::ofstream f(temp, std::ios::binary | std::ios::trunc);
        if (!f) return false;
        f << "AURA_TRACK_PRESET_V2\n";
        for (const auto& [name, c] : m_presets) {
            if (!valid_name(name) || !valid(c)) {
                f.close();
                std::filesystem::remove(temp);
                return false;
            }
            f << name << '\t' << c.gainDb << '\t' << c.faderDb << '\t' << c.pan << '\t'
              << c.mute << '\t' << c.solo << '\t' << c.phaseInvert << '\t'
              << c.groupId << '\t' << c.vcaId << '\t' << c.sends.size();
            for (const auto& send : c.sends) {
                f << '\t' << send.bus << '\t' << send.level << '\t' << send.pan << '\t'
                  << static_cast<unsigned>(send.tap) << '\t' << send.enabled;
            }
            f << '\n';
        }
        f.flush();
        if (!f) { std::filesystem::remove(temp); return false; }
        f.close();
        std::error_code error;
        std::filesystem::rename(temp, path, error);
        if (error) { std::filesystem::remove(temp); return false; }
        return true;
    }
    bool loadFile(const std::filesystem::path& path){if(!std::filesystem::is_regular_file(path))return false;std::ifstream f(path);if(!f)return false;std::unordered_map<std::string,ChannelConfiguration> next;std::string line;bool first=true;while(std::getline(f,line)){if(line.size()>65536)return false;if(first&&line=="AURA_TRACK_PRESET_V2"){first=false;continue;}first=false;std::vector<std::string>x;size_t p=0,q;while((q=line.find('\t',p))!=std::string::npos){x.push_back(line.substr(p,q-p));p=q+1;}x.push_back(line.substr(p));if(x.size()<6||!valid_name(x[0])||next.size()>=kMaxPresets)return false;try{ChannelConfiguration c;c.gainDb=std::stof(x[1]);c.faderDb=std::stof(x[2]);c.pan=std::stof(x[3]);c.mute=std::stoi(x[4])!=0;c.solo=std::stoi(x[5])!=0;if(x.size()>=9){c.phaseInvert=std::stoi(x[6])!=0;c.groupId=static_cast<uint32_t>(std::stoul(x[7]));c.vcaId=static_cast<uint32_t>(std::stoul(x[8]));}if(x.size()>=10){const size_t count=std::stoul(x[9]);if(count>64||x.size()!=10+count*5)return false;for(size_t i=0;i<count;++i){ChannelSend s;s.bus=static_cast<uint32_t>(std::stoul(x[10+i*5]));s.level=std::stof(x[11+i*5]);s.pan=std::stof(x[12+i*5]);s.tap=static_cast<SendTap>(std::min(2u,static_cast<unsigned>(std::stoul(x[13+i*5]))));s.enabled=std::stoi(x[14+i*5])!=0;c.sends.push_back(s);}}if(!valid(c)||!next.emplace(x[0],std::move(c)).second)return false;}catch(...){return false;}}m_presets.swap(next);return true;}
private:std::unordered_map<std::string,ChannelConfiguration>m_presets;
    static constexpr size_t kMaxPresets = 4096;
    static bool valid_name(const std::string& name){return !name.empty()&&name.size()<=256&&name.find_first_of("\t\r\n\0")==std::string::npos;}
    static bool valid(const ChannelConfiguration& c){if(!std::isfinite(c.gainDb)||!std::isfinite(c.faderDb)||!std::isfinite(c.pan)||c.gainDb<-120||c.gainDb>24||c.faderDb<-120||c.faderDb>24||c.pan<-1||c.pan>1||c.sends.size()>64)return false;for(const auto&s:c.sends)if(s.bus==0||!std::isfinite(s.level)||s.level<-120||s.level>24||!std::isfinite(s.pan)||s.pan<-1||s.pan>1)return false;return true;}
};
}
