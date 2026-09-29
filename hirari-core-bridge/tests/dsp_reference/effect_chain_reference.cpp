#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>

using HirariEffectNodeInfo = bool (*)(void*, uint32_t, bool*, bool*);
using HirariEffectNodeProcess = bool (*)(void*, uint32_t, bool, float*);

extern "C" void hirari_effect_chain_process_reference(
    void* userData,
    uint32_t nodeCount,
    float* const* channels,
    uint32_t channelCount,
    float* const* parallelChannels,
    uint32_t parallelChannelCount,
    uint32_t parallelCapacity,
    uint32_t frames,
    HirariEffectNodeInfo nodeInfo,
    HirariEffectNodeProcess processNode) {
    if (!channels || channelCount == 0 || frames == 0 || !nodeInfo || !processNode) return;
    for (uint32_t index = 0; index < nodeCount; ++index) {
        bool bypassed = false;
        bool parallel = false;
        if (!nodeInfo(userData, index, &bypassed, &parallel) || bypassed) continue;
        const bool useParallel = parallel && channelCount <= 2 &&
            channelCount <= parallelChannelCount && frames <= parallelCapacity && parallelChannels;
        if (useParallel) {
            for (uint32_t channel = 0; channel < channelCount; ++channel) {
                if (channels[channel] && parallelChannels[channel])
                    std::memcpy(parallelChannels[channel], channels[channel],
                                static_cast<size_t>(frames) * sizeof(float));
            }
        }
        float mix = 1.0f;
        if (!processNode(userData, index, useParallel, &mix)) {
            for (uint32_t channel = 0; channel < channelCount; ++channel) {
                if (channels[channel]) std::fill_n(channels[channel], frames, 0.0f);
            }
            continue;
        }
        if (useParallel) {
            mix = std::clamp(mix, 0.0f, 1.0f);
            for (uint32_t channel = 0; channel < channelCount; ++channel) {
                float* dry = channels[channel];
                const float* wet = parallelChannels[channel];
                if (!dry || !wet) continue;
                for (uint32_t frame = 0; frame < frames; ++frame)
                    dry[frame] = dry[frame] * (1.0f - mix) + wet[frame] * mix;
            }
        }
        for (uint32_t channel = 0; channel < channelCount; ++channel) {
            if (!channels[channel]) continue;
            for (uint32_t frame = 0; frame < frames; ++frame) {
                if (!std::isfinite(channels[channel][frame])) channels[channel][frame] = 0.0f;
            }
        }
    }
}
