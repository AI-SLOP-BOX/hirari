#pragma once
#include <algorithm>
#include <cstdint>
#include <cmath>

namespace Aura::Core::MIDI {
enum class MmcCommand : uint8_t { Stop=1, Play=2, RecordPunchIn=6, RecordPunchOut=7, Locate=68 };
struct MtcTime { uint8_t hours=0, minutes=0, seconds=0, frames=0; uint8_t fps=30; };
class ExternalSync {
public:
    void setClockRate(double bpm){if(std::isfinite(bpm)&&bpm>1.0&&bpm<1000.0)m_bpm=bpm;}
    double clockRate()const noexcept{return m_bpm;}
    void onClockTick(uint64_t timestamp){m_lastTick=timestamp; ++m_ticks;}
    uint64_t tickCount()const noexcept{return m_ticks;}
    uint64_t lastTick()const noexcept{return m_lastTick;}
    void reset(){m_ticks=0;m_lastTick=0;}
    static uint64_t samplesToMidiClocks(uint64_t samples,double sampleRate,double bpm){if(!std::isfinite(sampleRate)||sampleRate<=0||!std::isfinite(bpm)||bpm<=0)return 0;return static_cast<uint64_t>(std::llround(double(samples)*bpm*24.0/(sampleRate*60.0)));}
    static MtcTime framesToMtc(uint64_t frame,uint8_t fps=30){fps=std::clamp<uint8_t>(fps,1,120); MtcTime t; t.fps=fps; t.frames=frame%fps; frame/=fps; t.seconds=frame%60; frame/=60; t.minutes=frame%60; t.hours=frame/60; return t;}
private: double m_bpm=120.0; uint64_t m_ticks=0,m_lastTick=0;
};
}
