#pragma once
#include <vector>
#include <algorithm>
#include <string>
#include <memory>
#include <limits>
#include <cmath>
#include <atomic>
#include "../engine/timeline_system.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::IO {

/** Offline render facade. Rust owns block mixing, normalization, dither and
 * transactional stream writing; the callback below only invokes native track
 * rendering, which still depends on the engine's processor objects. */
class BounceEngine {
public:
    struct Options {
        std::string path;
        uint32_t sampleRate = 44100;
        uint32_t bitDepth = 24;
        bool normalize = false;
        bool dither = true;
        bool renderStems = false;
        uint32_t renderTap = 0;
        const std::atomic<bool>* cancellation = nullptr;
        uint32_t stemTrackId = 0;
    };

    void render(const Options& options, const Engine::TimelineSystem& timeline) {
        m_progress.store(0.0f);
        m_maxPeak = 0.0f;
        if (!hirari_bounce_options_valid(options.path.size(), options.sampleRate,
                                         options.bitDepth, options.renderTap)) return;

        const auto tracks = timeline.getTracksSnapshot();
        if (options.renderStems && options.stemTrackId == 0) {
            if (tracks.empty()) return;
            AudioBuffer block(2, 1024);
            RenderContext context{&timeline, &tracks, &block,
                static_cast<Engine::Track::OfflineRenderTap>(options.renderTap),
                options.cancellation, &m_progress, {}};
            (void)hirari_bounce_render_stems(
                reinterpret_cast<const uint8_t*>(options.path.data()), options.path.size(),
                options.sampleRate, options.bitDepth, options.normalize, options.dither,
                options.renderTap, timeline.getSelectionLength(), timeline.getBPM(),
                tracks.size(), &context, trackInfo, renderTrackBlock, cancelled,
                setProgress, &m_maxPeak);
            return;
        }

        const uint64_t totalSamples = hirari_bounce_total_samples(
            timeline.getSelectionLength(), timeline.getBPM(), options.sampleRate);
        if (totalSamples == 0) return;

        AudioBuffer block(2, 1024);
        std::vector<uint32_t> trackIds;
        trackIds.reserve(tracks.size());
        for (const auto& track : tracks) {
            if (track && (options.stemTrackId == 0 || track->getId() == options.stemTrackId))
                trackIds.push_back(track->getId());
        }
        RenderContext context{&timeline, &tracks, &block,
            static_cast<Engine::Track::OfflineRenderTap>(options.renderTap),
            options.cancellation, &m_progress, {}};
        float peak = 0.0f;
        const bool finished = hirari_bounce_render_stream(
            reinterpret_cast<const uint8_t*>(options.path.data()), options.path.size(),
            options.sampleRate, options.bitDepth, options.normalize, options.dither,
            totalSamples, trackIds.data(), trackIds.size(), &context,
            renderTrackBlock, cancelled, setProgress, &peak);
        m_maxPeak = peak;
        if (finished) m_progress.store(1.0f);
        else m_progress.store(0.0f);
    }

    float getProgress() const { return m_progress.load(); }
    float getMaxPeak() const { return m_maxPeak; }

private:
    struct RenderContext {
        const Engine::TimelineSystem* timeline;
        const std::vector<std::shared_ptr<Engine::Track>>* tracks;
        AudioBuffer* block;
        Engine::Track::OfflineRenderTap tap;
        const std::atomic<bool>* cancellation;
        std::atomic<float>* progress;
        std::string currentTrackName;
    };

    static bool trackInfo(void* opaque, size_t index, uint32_t* id,
                          const uint8_t** name, size_t* nameSize) {
        auto* context = static_cast<RenderContext*>(opaque);
        if (!id || !name || !nameSize || index >= context->tracks->size()) return false;
        const auto& track = (*context->tracks)[index];
        if (!track) return false;
        *id = track->getId();
        context->currentTrackName = track->getName();
        *name = reinterpret_cast<const uint8_t*>(context->currentTrackName.data());
        *nameSize = context->currentTrackName.size();
        return true;
    }

    static bool cancelled(void* opaque) {
        const auto* context = static_cast<const RenderContext*>(opaque);
        return context->cancellation && context->cancellation->load(std::memory_order_acquire);
    }
    static void setProgress(void* opaque, float value) {
        auto* context = static_cast<RenderContext*>(opaque);
        context->progress->store(value, std::memory_order_release);
    }
    static bool renderTrackBlock(void* opaque, uint32_t trackId, uint64_t start,
                                 float* left, float* right, uint32_t count) {
        auto* context = static_cast<RenderContext*>(opaque);
        context->block->clear(count);
        if (!context->timeline->renderTrackInto(trackId, *context->block, count, start, context->tap))
            return false;
        const float* sourceLeft = context->block->getReadPointer(0);
        const float* sourceRight = context->block->getReadPointer(1);
        if (!sourceLeft || !sourceRight) return false;
        std::copy_n(sourceLeft, count, left);
        std::copy_n(sourceRight, count, right);
        return true;
    }

    std::atomic<float> m_progress{0.0f};
    float m_maxPeak = 0.0f;
};

} // namespace Hirari::Core::IO
