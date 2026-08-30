#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <algorithm>
#include <cmath>
#include <cstring>

namespace Aura::Core::Video {

/**
 * @struct OverlayCue
 * @brief Representation of a visual cue (Streamer, Punch, Text) over video.
 */
struct OverlayCue {
    enum class Type { Streamer, Punch, Text, Timecode };
    Type type;
    uint64_t startSample;
    uint64_t durationSamples;
    float color[4];
    char text[128];
};

/**
 * @class VideoOverlayEngine
 * @brief Professional overlay and cueing engine for scoring and ADR.
 * Provides frame-accurate visual orchestration for cinematic production.
 */
class VideoOverlayEngine {
public:
    VideoOverlayEngine() = default;

    void addCue(const OverlayCue& cue) {
        if (cue.durationSamples == 0 || !std::isfinite(cue.color[0]) ||
            !std::isfinite(cue.color[1]) || !std::isfinite(cue.color[2]) ||
            !std::isfinite(cue.color[3])) return;
        m_cues.push_back(cue);
        std::stable_sort(m_cues.begin(), m_cues.end(), [](const OverlayCue& a, const OverlayCue& b) {
            return a.startSample < b.startSample;
        });
    }

    void clearCues() noexcept { m_cues.clear(); }
    size_t cueCount() const noexcept { return m_cues.size(); }

    /**
     * @brief Render the cues for the current audio position.
     */
    void render(uint64_t currentSample, void* targetBuffer, uint32_t width, uint32_t height) {
        if (!targetBuffer || width == 0 || height == 0) return;
        // Target is a caller-owned packed RGBA8 buffer.  This path is used by
        // offline/video preview rendering and never touches the audio thread.
        for (const auto& cue : m_cues) {
            const uint64_t end = cue.startSample > UINT64_MAX - cue.durationSamples
                ? UINT64_MAX : cue.startSample + cue.durationSamples;
            if (currentSample >= cue.startSample && currentSample < end) {
                renderCue(cue, currentSample, targetBuffer, width, height);
            }
        }
    }

private:
    void renderCue(const OverlayCue& cue, uint64_t currentSample, void* target, uint32_t w, uint32_t h) {
        auto* pixels = static_cast<uint32_t*>(target);
        const auto pack = [&cue](float alpha) {
            const auto c = [](float v) -> uint32_t {
                return static_cast<uint32_t>(std::clamp(v, 0.0f, 1.0f) * 255.0f + 0.5f);
            };
            return (c(alpha * cue.color[3]) << 24) | (c(cue.color[2]) << 16) |
                   (c(cue.color[1]) << 8) | c(cue.color[0]);
        };
        const auto blend = [pixels, w](uint32_t x, uint32_t y, uint32_t src) {
            if (x >= w) return;
            const uint32_t alpha = src >> 24;
            if (alpha == 0) return;
            uint32_t& dst = pixels[static_cast<size_t>(y) * w + x];
            if (alpha == 255) { dst = src; return; }
            const uint32_t inv = 255 - alpha;
            const uint32_t r = ((src & 0xffu) * alpha + (dst & 0xffu) * inv) / 255;
            const uint32_t g = (((src >> 8) & 0xffu) * alpha + ((dst >> 8) & 0xffu) * inv) / 255;
            const uint32_t b = (((src >> 16) & 0xffu) * alpha + ((dst >> 16) & 0xffu) * inv) / 255;
            dst = 0xff000000u | r | (g << 8) | (b << 16);
        };
        if (cue.type == OverlayCue::Type::Punch) {
            const double progress = static_cast<double>(currentSample - cue.startSample) /
                                    static_cast<double>(std::max<uint64_t>(1, cue.durationSamples));
            const float alpha = static_cast<float>(1.0 - std::clamp(progress, 0.0, 1.0));
            const uint32_t color = pack(alpha * 0.7f);
            for (uint32_t y = 0; y < h; ++y)
                for (uint32_t x = 0; x < w; ++x) blend(x, y, color);
        } else if (cue.type == OverlayCue::Type::Streamer) {
            const double progress = static_cast<double>(currentSample - cue.startSample) /
                                    static_cast<double>(std::max<uint64_t>(1, cue.durationSamples));
            const uint32_t x = static_cast<uint32_t>(std::clamp(progress, 0.0, 1.0) * (w - 1));
            const uint32_t color = pack(0.9f);
            for (uint32_t y = 0; y < h; ++y) {
                blend(x, y, color);
                if (x > 0) blend(x - 1, y, pack(0.25f));
            }
        } else if (cue.type == OverlayCue::Type::Timecode) {
            // Timecode is represented by a top/bottom guide in the pixel-only
            // backend; native UI backends can render cue.text separately.
            const uint32_t color = pack(0.85f);
            for (uint32_t x = 0; x < w; ++x) { blend(x, 0, color); blend(x, h - 1, color); }
        }
    }

    std::vector<OverlayCue> m_cues;
};

} // namespace Aura::Core::Video
