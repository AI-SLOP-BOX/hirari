#pragma once
#include <vector>
#include <array>
#include <string>
#include <algorithm>
#include <filesystem>
#include <atomic>
#include <cmath>
#include <memory>
#include <utility>
#include <mutex>
#include <thread>
#include <chrono>
#include "../../dsp/mixing/channel_strip.hpp"
#include "region_processor.hpp"
#include "../audio_buffer.hpp"
#include "../project_serializer.hpp"
#include "../io/audio_decoder.hpp"
#include "automation_recorder.hpp"
#include "../effect_chain.hpp"
#include "../midi_region.hpp"
#include "../plugin_host.hpp"
#include "../plugins/process_sandbox_processor.hpp"
#include "../plugins/plugin_cache_manager.hpp"
#include "../../core/dsp/spatial/holographic_panner.hpp"
#include "vca_manager.hpp"
#include "../editing/audio_note_segment.hpp"
#include "../editing/audio_pitch_analysis.hpp"
#include "../editing/event_processing_history.hpp"

namespace Aura::Core::Engine {

struct Region {
    uint32_t id;
    std::string path;
    uint64_t start;
    uint64_t len;
    uint64_t sourceOffset = 0;
    uint64_t baseStart = 0;
    uint64_t baseSourceOffset = 0;
    uint64_t baseLength = 0;
    bool muted;
    std::string name;
    std::shared_ptr<AudioBuffer> audio;
    float clipGain = 1.0f;
    uint64_t fadeInSamples = 64;
    uint64_t fadeOutSamples = 64;
    bool reverse = false;
    double warpRatio = 1.0;
    float pitchSemitones = 0.0f;
    // Non-destructive VariAudio-style note edits for this audio region.
    std::vector<::aura::editing::AudioNoteSegment> audioNoteSegments;
    uint32_t loopCount = 1;
    bool locked = false;
    uint32_t syncGroup = 0;
    std::vector<::aura::editing::EventProcessingStep> processingHistory;
    // Derived on the control thread when a region snapshot is published.
    uint64_t crossfadeInSamples = 0;
    uint64_t crossfadeOutSamples = 0;
};

/**
 * @class Track
 * @brief High-performance track orchestration engine.
 * HONEST FIX: Optimized region lookup using sorted-list binary search.
 */
class Track {
    #include "track_part_1.inc"
    #include "track_part_2.inc"
    #include "track_part_3.inc"
    #include "track_part_4.inc"

} // namespace Aura::Core::Engine
