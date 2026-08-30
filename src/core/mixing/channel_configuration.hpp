#pragma once

#include <algorithm>
#include <cstdint>
#include <string>
#include <unordered_map>
#include <vector>
#include <cmath>

namespace Aura::Core::Mixing {

enum class SendTap : uint8_t { PreFader, PostFader, PrePan };
struct ChannelSend { uint32_t bus = 0; float level = 0.0f; float pan = 0.0f; SendTap tap = SendTap::PostFader; bool enabled = true; };
struct ChannelConfiguration {
    float gainDb = 0.0f, faderDb = 0.0f, pan = 0.0f;
    bool mute = false, solo = false, phaseInvert = false;
    uint32_t groupId = 0, vcaId = 0;
    std::vector<ChannelSend> sends;
};

class ChannelConfigurationStore {
public:
    bool set(uint32_t channel, ChannelConfiguration config) {
        if (channel == 0 || !valid(config)) return false;
        m_channels[channel] = std::move(config); return true;
    }
    const ChannelConfiguration* get(uint32_t channel) const { auto it=m_channels.find(channel); return it==m_channels.end()?nullptr:&it->second; }
    bool copy(uint32_t source, uint32_t destination) {
        const auto* c=get(source); if(!c || destination==0 || !valid(*c)) return false; m_channels[destination]=*c; return true;
    }
    bool setSend(uint32_t channel, ChannelSend send) {
        auto* c=mutableGet(channel); if(!c || send.bus==0 || send.level < -120.0f || send.level > 24.0f) return false;
        if (!std::isfinite(send.level) || !std::isfinite(send.pan) || send.pan < -1.0f || send.pan > 1.0f) return false;
        auto it=std::find_if(c->sends.begin(),c->sends.end(),[&](const auto& x){return x.bus==send.bus;}); if(it==c->sends.end()) { if(c->sends.size()>=64) return false; c->sends.push_back(send); } else *it=send; return true;
    }
    bool linkGroup(uint32_t channel, uint32_t group) { auto*c=mutableGet(channel); if(!c) return false; c->groupId=group; return true; }
    bool linkVca(uint32_t channel, uint32_t vca) { auto*c=mutableGet(channel); if(!c) return false; c->vcaId=vca; return true; }
private:
    ChannelConfiguration* mutableGet(uint32_t id){auto it=m_channels.find(id);return it==m_channels.end()?nullptr:&it->second;}
    static bool valid(const ChannelConfiguration& c){return std::isfinite(c.gainDb)&&std::isfinite(c.faderDb)&&std::isfinite(c.pan)&&c.gainDb>=-120&&c.gainDb<=24&&c.faderDb>=-120&&c.faderDb<=24&&c.pan>=-1&&c.pan<=1&&c.sends.size()<=64;}
    std::unordered_map<uint32_t,ChannelConfiguration> m_channels;
};
}
