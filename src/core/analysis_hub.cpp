#include "analysis_hub.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include "hirari_unified_engine.hpp"
#include "neural_bridge.hpp"
#include <algorithm>

namespace Hirari::Core::BridgeFFI {

    rust::Vec<float> AnalysisHub::get_mixing_advice_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        const auto meter = m_engine->getMasterMeterData();
        return ::Hirari::Core::Bridge::analysis_mixing_advice(
            static_cast<float>(meter.truePeakL), static_cast<float>(meter.truePeakR),
            static_cast<float>(meter.correlation));
    }

    rust::String AnalysisHub::get_arrangement_advice(uint32_t trackId) const {
        return ::Hirari::Core::Bridge::analysis_arrangement_advice(trackId);
    }

    rust::Vec<float> AnalysisHub::get_imaging_data_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        result.reserve(2);
        auto data = m_engine->getMasterMeterData();
        result.push_back((float)data.correlation);
        result.push_back((float)data.balance);
        return result;
    }

    rust::Vec<float> AnalysisHub::get_spectral_data_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        const double sampleRate = std::clamp(m_engine->get_sample_rate(), 8000.0, 384000.0);
        return ::Hirari::Core::Bridge::analysis_spectral_display(
            rust::Slice<const float>(telemetry.spectrum, 512), sampleRate);
    }

    rust::Vec<float> AnalysisHub::get_mixer_levels_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        // Copy through the seqlock-protected bridge helpers. Returning a
        // borrowed slice here lets the audio callback overwrite the inactive
        // telemetry buffer while this analysis thread is still iterating.
        auto l = ::Hirari::Core::Bridge::get_track_peaks_l_owned(*m_engine);
        auto r = ::Hirari::Core::Bridge::get_track_peaks_r_owned(*m_engine);
        
        const size_t count = std::min(l.size(), r.size());
        result.reserve(count * 2);
        for(size_t i=0; i < count; ++i) {
             result.push_back(l[i]);
             result.push_back(r[i]);
        }
        return result;
    }

    std::vector<::Hirari::Core::Bridge::PlainClash> AnalysisHub::get_spectral_clash_v() const {
        std::vector<::Hirari::Core::Bridge::PlainClash> result;
        if (!m_engine) return result;
        const auto meter = m_engine->getMasterMeterData();
        const auto encoded = ::Hirari::Core::Bridge::analysis_spectral_clashes(
            static_cast<float>(meter.truePeakL), static_cast<float>(meter.truePeakR),
            static_cast<float>(meter.correlation));
        for (size_t index = 0; index + 1 < encoded.size(); index += 2) {
            result.push_back({static_cast<uint32_t>(encoded[index]), encoded[index + 1], 0u});
        }
        return result;
    }

    ::Hirari::Core::Bridge::PlainLoudness AnalysisHub::get_master_loudness_v() const {
        if (!m_engine) return {};
        auto data = m_engine->getMasterMeterData();
        
        // Update Loudness History
        {
            std::lock_guard<std::mutex> lock(m_loudnessHistoryMutex);
            if (m_loudnessHistory.size() >= 1024) m_loudnessHistory.erase(m_loudnessHistory.begin());
            m_loudnessHistory.push_back((float)data.lufsShortTerm);
        }

        return {
            (float)data.lufsIntegrated,
            (float)data.lufsShortTerm,
            (float)data.truePeakL,
            (float)data.truePeakR,
            (float)data.correlation
        };
    }

    rust::Vec<float> AnalysisHub::get_loudness_history_v() const {
        rust::Vec<float> result;
        std::lock_guard<std::mutex> lock(m_loudnessHistoryMutex);
        result.reserve(m_loudnessHistory.size());
        for (const float value : m_loudnessHistory) result.push_back(value);
        return result;
    }

    rust::Vec<float> AnalysisHub::get_phase_heatmap_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        return ::Hirari::Core::Bridge::analysis_phase_heatmap(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

    rust::String AnalysisHub::get_intelligence_dashboard_json() const { 
        if (!m_engine) return rust::String("{\"available\":false}");
        const auto meter = m_engine->getMasterMeterData();
        return ::Hirari::Core::Bridge::analysis_dashboard_json(
            static_cast<float>(meter.lufsShortTerm), static_cast<float>(meter.lufsIntegrated),
            static_cast<float>(meter.truePeakL), static_cast<float>(meter.truePeakR),
            static_cast<float>(meter.correlation));
    }

    void AnalysisHub::trigger_background_analysis() const {
        if (!m_engine) return;

        // Keep this call real-time friendly: the bridge only enqueues a compact
        // advice packet in its lock-free queue; expensive analysis stays off the
        // caller's (potentially audio) thread.
        const auto meter = m_engine->getMasterMeterData();
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return;
        const float truePeak = std::max(static_cast<float>(meter.truePeakL),
                                        static_cast<float>(meter.truePeakR));
        ::Hirari::Core::AI::NeuralBridge::getInstance().evaluateSignal(
            static_cast<float>(meter.lufsIntegrated), truePeak, telemetry.spectrum);
    }

    rust::String AnalysisHub::get_song_structure_json() const {
        if (!m_engine) return rust::String("{\"available\":false,\"sections\":[]}");
        const auto sections = m_engine->run_structural_analysis();
        std::vector<uint8_t> kinds;
        std::vector<uint64_t> starts, ends;
        std::vector<float> energies, flows;
        kinds.reserve(sections.size());
        starts.reserve(sections.size());
        ends.reserve(sections.size());
        energies.reserve(sections.size());
        flows.reserve(sections.size());
        for (const auto& section : sections) {
            kinds.push_back(section.section_type);
            starts.push_back(section.start_sample);
            ends.push_back(section.end_sample);
            energies.push_back(section.energy_level);
            flows.push_back(section.narrative_flow_score);
        }
        return ::Hirari::Core::Bridge::analysis_song_structure_json(
            rust::Slice<const uint8_t>(kinds.data(), kinds.size()),
            rust::Slice<const uint64_t>(starts.data(), starts.size()),
            rust::Slice<const uint64_t>(ends.data(), ends.size()),
            rust::Slice<const float>(energies.data(), energies.size()),
            rust::Slice<const float>(flows.data(), flows.size()));
    }

    rust::Vec<float> AnalysisHub::get_spectral_partials_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        return ::Hirari::Core::Bridge::analysis_partials(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

    rust::String AnalysisHub::get_creative_advice() const {
        if (!m_engine) return rust::String("Analysis unavailable.");
        const auto meter = m_engine->getMasterMeterData();
        return ::Hirari::Core::Bridge::analysis_creative_advice(
            static_cast<float>(meter.truePeakL), static_cast<float>(meter.truePeakR),
            static_cast<float>(meter.correlation));
    }

    rust::Vec<float> AnalysisHub::get_mel_spectrogram_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        return ::Hirari::Core::Bridge::analysis_mel_spectrogram(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

    float AnalysisHub::get_phase_correlation() const {
        if (!m_engine) return 0.0f;
        return std::clamp(static_cast<float>(m_engine->getMasterMeterData().correlation), -1.0f, 1.0f);
    }

    rust::Vec<float> AnalysisHub::get_motion_vectors_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        return ::Hirari::Core::Bridge::analysis_motion_vectors(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

    float AnalysisHub::get_motion_energy() const {
        if (!m_engine) return 0.0f;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return 0.0f;
        return ::Hirari::Core::Bridge::analysis_motion_energy(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

    rust::Vec<float> AnalysisHub::get_synesthesia_colors_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(m_engine->get_active_telemetry_idx(), telemetry)) return result;
        return ::Hirari::Core::Bridge::analysis_synesthesia_colors(
            rust::Slice<const float>(telemetry.spectrum, 512));
    }

} // namespace Hirari::Core::BridgeFFI
