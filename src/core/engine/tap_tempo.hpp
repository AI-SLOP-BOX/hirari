#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <deque>

namespace Aura::Core::Engine {
class TapTempo {
public:
    explicit TapTempo(size_t maxTaps=8):m_max(std::max<size_t>(2,maxTaps)){}
    void tap(uint64_t timestampMs){if(!m_taps.empty()&&timestampMs<=m_taps.back())return;m_taps.push_back(timestampMs);while(m_taps.size()>m_max)m_taps.pop_front();}
    void clear(){m_taps.clear();}
    double bpm()const noexcept{if(m_taps.size()<2)return 0;double sum=0;size_t n=0;for(size_t i=1;i<m_taps.size();++i){const auto d=double(m_taps[i]-m_taps[i-1]);if(d>=100.0&&d<=5000.0){sum+=d;++n;}}return n?std::clamp(60000.0/(sum/n),20.0,300.0):0;}
    size_t count()const noexcept{return m_taps.size();}
private:size_t m_max;std::deque<uint64_t>m_taps;
};
}
