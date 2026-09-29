#pragma once

#include <vector>
#include <memory>
#include <atomic>
#include <chrono>
#include <string_view>
#include <array>
#include "audio_buffer.hpp"
#include "midi_buffer.hpp"
#include "preview_synth.hpp"
#include "engine_types.hpp"
#include "bridge_types.hpp"
#include "engine_types.hpp"
#include "utils/ring_buffer.hpp"
#include "concurrency/audio_task_manager.hpp"
#include "engine/pdc_manager.hpp"
#include "engine/tonal_sync.hpp"
#include "engine/tempo_map.hpp"
#include "engine/routing_engine.hpp"
#include "engine/undo_transaction_manager.hpp"
#include "editing/audio_note_segment.hpp"
#include "engine/sidechain_manager.hpp"
#include "engine/track_freeze_manager.hpp"
#include "engine/bus_system.hpp"
#include "engine/bus_track.hpp"
#include "engine/mixer_telemetry.hpp"
#include "engine/macro_control_manager.hpp"
#include "engine/vca_manager.hpp"
#include "midi_learn_manager.hpp"
#include "engine/metronome.hpp"
#include "engine/automation_recorder.hpp"
#include "engine/midi_orchestrator.hpp"
#include "engine/mpe_manager.hpp"
#include "diagnostics/engine_diagnostics.hpp"
#include "diagnostics/forensic_journaler.hpp"
#include "io/audio_decoder.hpp"
#include "../dsp/utils/dither.hpp"
#include "plugins/plugin_compatibility_registry.hpp"
#include <unordered_map>
#include <unordered_set>
#include <mutex>
#include <condition_variable>
#include <map>
#include <algorithm>
#include <iterator>
#include <cmath>
#include <cctype>
#include <fstream>
#include <filesystem>
#include <ostream>
#include <array>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

// Forward declarations
namespace Hirari::DSP { struct ProcessContext; }
namespace Hirari::DSP::Analysis { class MasterMeter; }
namespace Hirari::DSP::Effects { class MasterLimiter; }

namespace Hirari::Core::Video { class VideoSync; }

#include "log_buffer.hpp"
#include "diagnostics/engine_diagnostics.hpp"
#include "engine/script_manager.hpp"
#include "engine/macro_control_manager.hpp"
#include "engine/routing_engine.hpp"
#include "dsp/effects/atmospheric_processor_kernel.hpp"
#include "mixing/mastering_kernel.hpp"
#include "mixing/control_room.hpp"
#include "mixing/track_preset_store.hpp"
#include "engine/automation_lane_group.hpp"
#include "engine/tap_tempo.hpp"
#include "engine/audio_quantizer.hpp"
#include "midi/external_sync.hpp"
#include "composition/arrangement_edit_model.hpp"
#include "composition/arrangement_section.hpp"
#include "composition/harmonic_context_tracker.hpp"
#include <thread>
#include <future>

namespace Hirari::Core::Engine {

class ScriptManager;
class Track;

namespace Mixing {
struct AestheticFeatures {
    float transientClarity = 1.0f;
    float dynamicComplexity = 0.0f;
};

struct QualitativeMetricEngine {
    static QualitativeMetricEngine& getInstance() {
        static QualitativeMetricEngine instance;
        return instance;
    }

    std::map<std::string, float> calculateScores(const AestheticFeatures& features) const {
        const float clarity = std::clamp(features.transientClarity, 0.0f, 1.0f);
        const float complexity = std::clamp(features.dynamicComplexity, 0.0f, 1.0f);
        return {
            {"transient_clarity", clarity},
            {"dynamic_complexity", complexity},
            {"balance", (clarity + (1.0f - std::fabs(complexity - 0.5f) * 2.0f)) * 0.5f},
            {"energy", complexity}
        };
    }
};

} // namespace Mixing

struct EngineConfig {
    float tempo;
    uint32_t timeSigNum;
    uint32_t timeSigDen;
    uint32_t sampleRate;
    uint32_t blockSize;
};

class HirariUnifiedEngine {
#include "hirari_unified_engine_decl_part_1.inc"
#include "hirari_unified_engine_decl_part_2.inc"
#include "hirari_unified_engine_decl_part_3.inc"
#include "hirari_unified_engine_decl_part_4.inc"
#include "hirari_unified_engine_decl_part_5.inc"
#include "hirari_unified_engine_decl_part_6.inc"
#include "hirari_unified_engine_decl_part_7.inc"
#include "hirari_unified_engine_decl_part_8.inc"

} // namespace Hirari::Core::Engine
