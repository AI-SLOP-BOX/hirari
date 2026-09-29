#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

struct HirariRegionRangeEdit {
    uint64_t start;
    uint64_t end;
    float gain;
    uint64_t fadeIn;
    uint64_t fadeOut;
};

struct HirariRegionCompRange {
    uint64_t start;
    uint64_t end;
    uint64_t fadeIn;
    uint64_t fadeOut;
};

extern "C" void hirari_region_gain_reference(
    float* output, uint32_t frames, uint64_t regionOffset,
    uint64_t regionLength, uint64_t timelineLength,
    uint64_t fadeIn, uint64_t fadeOut, uint64_t crossfadeIn,
    uint64_t crossfadeOut, float clipGain, uint8_t compManaged,
    const HirariRegionCompRange* compRanges, size_t compRangeCount,
    const HirariRegionRangeEdit* rangeEdits, size_t rangeEditCount) {
    if (output == nullptr || frames == 0 || regionLength == 0 || !std::isfinite(clipGain)) return;
    for (uint32_t index = 0; index < frames; ++index) {
        const uint64_t relative = regionOffset + index;
        const uint64_t loopRelative = relative % regionLength;
        const HirariRegionCompRange* compRange = nullptr;
        if (compManaged != 0) {
            size_t low = 0;
            size_t high = compRangeCount;
            while (low < high) {
                const size_t middle = low + (high - low) / 2;
                if (compRanges[middle].start <= loopRelative) low = middle + 1;
                else high = middle;
            }
            if (low == 0) { output[index] = 0.0f; continue; }
            compRange = &compRanges[low - 1];
            if (loopRelative < compRange->start || loopRelative >= compRange->end) {
                output[index] = 0.0f;
                continue;
            }
        }

        float fade = 1.0f;
        if (fadeIn > 0 && relative < fadeIn)
            fade = static_cast<float>(relative) / static_cast<float>(fadeIn);
        if (fadeOut > 0 && timelineLength > fadeOut && relative >= timelineLength - fadeOut)
            fade = std::min(fade, static_cast<float>(timelineLength - relative) / static_cast<float>(fadeOut));
        if (crossfadeIn > 0 && relative < crossfadeIn)
            fade = std::min(fade, static_cast<float>(relative) / static_cast<float>(crossfadeIn));
        if (crossfadeOut > 0 && relative >= timelineLength - crossfadeOut)
            fade = std::min(fade, static_cast<float>(timelineLength - relative) / static_cast<float>(crossfadeOut));

        float rangeGain = 1.0f;
        float rangeFade = 1.0f;
        bool rangeMuted = false;
        for (size_t editIndex = 0; editIndex < rangeEditCount; ++editIndex) {
            const auto& edit = rangeEdits[editIndex];
            if (edit.start > loopRelative) break;
            if (edit.start >= edit.end || loopRelative < edit.start || loopRelative >= edit.end ||
                !std::isfinite(edit.gain) || edit.gain < 0.0f || edit.gain > 16.0f) continue;
            rangeGain = std::min(rangeGain * edit.gain, 16.0f);
            if (!std::isfinite(rangeGain)) rangeGain = 16.0f;
            const uint64_t length = edit.end - edit.start;
            if (edit.fadeIn > 0 && edit.fadeIn <= length && loopRelative - edit.start < edit.fadeIn)
                rangeFade = std::min(rangeFade, static_cast<float>(loopRelative - edit.start) / static_cast<float>(edit.fadeIn));
            if (edit.fadeOut > 0 && edit.fadeOut <= length && loopRelative - edit.start >= length - edit.fadeOut)
                rangeFade = std::min(rangeFade, static_cast<float>(edit.end - loopRelative) / static_cast<float>(edit.fadeOut));
            if (edit.gain == 0.0f) rangeMuted = true;
        }
        if (rangeMuted) { output[index] = 0.0f; continue; }
        if (!std::isfinite(rangeFade)) rangeFade = 1.0f;
        float compFade = 1.0f;
        if (compRange != nullptr) {
            if (compRange->fadeIn > 0 && loopRelative - compRange->start < compRange->fadeIn) {
                const float phase = static_cast<float>(loopRelative - compRange->start + 1) / static_cast<float>(compRange->fadeIn);
                compFade = std::sin(std::clamp(phase, 0.0f, 1.0f) * 1.57079632679f);
            }
            if (compRange->fadeOut > 0 && compRange->end - loopRelative <= compRange->fadeOut) {
                const float phase = static_cast<float>(compRange->end - loopRelative) / static_cast<float>(compRange->fadeOut);
                compFade = std::min(compFade, std::sin(std::clamp(phase, 0.0f, 1.0f) * 1.57079632679f));
            }
        }
        output[index] = clipGain * std::clamp(fade * rangeFade * rangeGain * compFade, 0.0f, 16.0f);
    }
}
