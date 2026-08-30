#pragma once
#include <memory>
#include <mutex>
#include <utility>
#include <vector>
#include "rust/cxx.h"
#include "bridge_types.hpp"

namespace Aura::Core::Engine { class AuraUnifiedEngine; }

namespace Aura::Core::Bridge {
bool initialize_gpu_with_status();
rust::Vec<float> get_track_peaks_l_owned(const ::Aura::Core::Engine::AuraUnifiedEngine& engine);
rust::Vec<float> get_track_peaks_r_owned(const ::Aura::Core::Engine::AuraUnifiedEngine& engine);
}

namespace Aura::Core::BridgeFFI {

struct StructureNode;

/**
 * @class AnalysisHub
 * @brief Professional Analysis Bridge for Aura Studio Pro.
 */
class AnalysisHub {
public:
    AnalysisHub(std::shared_ptr<::Aura::Core::Engine::AuraUnifiedEngine> engine) 
        : m_engine(std::move(engine)) {}

    rust::Vec<float> get_mixing_advice_v() const;
    // Native container only; the CXX boundary owns conversion to rust::Vec.
    std::vector<::Aura::Core::Bridge::PlainClash> get_spectral_clash_v() const;
    void trigger_background_analysis() const;
    ::Aura::Core::Bridge::PlainLoudness get_master_loudness_v() const;
    rust::String get_arrangement_advice(uint32_t track_id) const;
    rust::Vec<float> get_imaging_data_v() const;
    rust::String get_song_structure_json() const;
    rust::Vec<float> get_spectral_data_v() const;
    rust::Vec<float> get_spectral_partials_v() const;
    rust::String get_creative_advice() const;
    rust::Vec<float> get_mixer_levels_v() const;
    rust::Vec<float> get_mel_spectrogram_v() const;
    float get_phase_correlation() const;
    rust::Vec<float> get_phase_heatmap_v() const;
    rust::Vec<float> get_loudness_history_v() const;
    rust::Vec<float> get_motion_vectors_v() const;
    float get_motion_energy() const;
    rust::Vec<float> get_synesthesia_colors_v() const;
    rust::String get_intelligence_dashboard_json() const;

private:
    std::shared_ptr<::Aura::Core::Engine::AuraUnifiedEngine> m_engine;
    
    mutable std::vector<float> m_loudnessHistory;
    mutable std::mutex m_loudnessHistoryMutex;
};

} // namespace Aura::Core::BridgeFFI
