#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>

namespace Aura::Core::Mixing {

struct MeterSnapshot { float peakL=0, peakR=0, rmsL=0, rmsR=0, loudnessLUFS=-INFINITY, correlation=0, gainDb=0; std::vector<float> spectrum; };

class MixMetering {
public:
    explicit MixMetering(size_t bands = 24) : m_bands(std::max<size_t>(1, bands)) {}
    MeterSnapshot process(const float* left, const float* right, uint32_t frames, float gainDb = 0) const noexcept {
        MeterSnapshot s; s.spectrum.assign(m_bands, 0.0f); s.gainDb = std::isfinite(gainDb) ? gainDb : 0.0f;
        if (!left || !right || frames == 0) return s;
        double eL=0,eR=0,cross=0,den=0; const float gain=std::pow(10.0f,s.gainDb/20.0f);
        for(uint32_t i=0;i<frames;++i){ const float l=left[i]*gain,r=right[i]*gain; s.peakL=std::max(s.peakL,std::fabs(l)); s.peakR=std::max(s.peakR,std::fabs(r)); eL+=double(l)*l; eR+=double(r)*r; cross+=double(l)*r; den+=double(l)*l+double(r)*r; }
        s.rmsL=std::sqrt(static_cast<float>(eL/frames)); s.rmsR=std::sqrt(static_cast<float>(eR/frames));
        s.correlation=den>1e-12?static_cast<float>(2.0*cross/den):0.0f;
        const double mean=(eL+eR)/(2.0*frames); s.loudnessLUFS=mean>1e-12?static_cast<float>(-0.691+10.0*std::log10(mean)):-INFINITY;
        for(size_t b=0;b<m_bands;++b){ double sum=0; const double omega=3.141592653589793*(b+1)/m_bands; for(uint32_t i=0;i<frames;++i){ const double x=0.5*(left[i]+right[i])*gain; sum+=x*std::cos(omega*i); } s.spectrum[b]=static_cast<float>(std::fabs(sum/frames)); }
        return s;
    }
private: size_t m_bands;
};
}
