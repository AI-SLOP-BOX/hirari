#pragma once
#include <algorithm>
#include <cstdint>
#include <random>
#include <vector>

namespace Aura::Core::MIDI {
struct TransformNote { uint64_t start=0,length=1; uint8_t pitch=0,velocity=0; float probability=1.0f; };
class NoteTransformer {
public:
    static void scaleLength(std::vector<TransformNote>& n,float factor){if(factor<=0||!std::isfinite(factor))return;for(auto&x:n)x.length=std::max<uint64_t>(1,static_cast<uint64_t>(x.length*factor));}
    static void adjustVelocity(std::vector<TransformNote>& n,int delta){for(auto&x:n)x.velocity=static_cast<uint8_t>(std::clamp<int>(int(x.velocity)+delta,0,127));}
    static void setProbability(std::vector<TransformNote>& n,float p){if(!std::isfinite(p))return;for(auto&x:n)x.probability=std::clamp(p,0.0f,1.0f);}
    static std::vector<TransformNote> repeat(const std::vector<TransformNote>& n,uint32_t count,uint64_t spacing){std::vector<TransformNote>o;if(count==0)return o;o.reserve(n.size()*count);for(uint32_t i=0;i<count;++i)for(auto x:n){x.start+=spacing*i;o.push_back(x);}return o;}
    static void applyProbability(std::vector<TransformNote>& n,uint32_t seed){std::mt19937 g(seed);std::uniform_real_distribution<float>d(0,1);n.erase(std::remove_if(n.begin(),n.end(),[&](const auto&x){return d(g)>std::clamp(x.probability,0.0f,1.0f);}),n.end());}
};
}
