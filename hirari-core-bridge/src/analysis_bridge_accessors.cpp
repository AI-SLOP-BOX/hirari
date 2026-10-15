/* Hirari Core Bridge - analysis accessor implementations.
 *
 * These are the C++ side of the AnalysisHub extinction: the BridgeFFI::AnalysisHub
 * layer (src/core/analysis_hub.cpp/.hpp) is deleted; the Rust callers switched to
 * these accessors, which read the authoritative engine (HirariUnifiedEngine) and
 * the pure-Rust presentation transforms in analysis_views.rs.
 *
 * Implementation of cxx-generated `extern "C++"` symbols declared in
 * hirari-core-bridge/src/lib.rs (`pub mod ffi`).
 */
#include "hirari-core-bridge/src/lib.rs.h"

#include <cstdint>
#include <vector>

namespace Hirari::Core::Bridge {

// The authoritative engine telemetry snapshot (peaks, 512-bin spectrum, meter).
uint32_t get_active_telemetry_idx(const Engine::HirariUnifiedEngine& engine) {
    return engine.get_active_telemetry_idx();
}

rust::Vec<float> get_spectrum(const Engine::HirariUnifiedEngine& engine, uint32_t idx) {
    ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData snapshot{};
    if (!engine.copy_telemetry(idx, snapshot)) return {};
    rust::Vec<float> result;
    result.reserve(512);
    for (uint32_t i = 0; i < 512; ++i) result.push_back(snapshot.spectrum[i]);
    return result;
}

// Current meter (true peak, correlation, integrated/short-term loudness).
BridgeLoudness get_master_meter_data(const Engine::HirariUnifiedEngine& engine) {
    const auto meter = engine.getMasterMeterData();
    return BridgeLoudness{
        .integrated = meter.lufsIntegrated,
        .short_term = meter.lufsShortTerm,
        .true_peak_l = meter.truePeakL,
        .true_peak_r = meter.truePeakR,
        .correlation = meter.correlation,
    };
}

// Intelligence dashboard JSON (meter only; the deleted AnalysisHub computed the
// identical payload from the same engine fields).
rust::String get_intelligence_dashboard_json(const Engine::HirariUnifiedEngine& engine) {
    const auto meter = engine.getMasterMeterData();
    return ::Hirari::Core::Bridge::analysis_dashboard_json(
        meter.lufsShortTerm, meter.lufsIntegrated,
        meter.truePeakL, meter.truePeakR, meter.correlation);
}

// Song structure JSON (sections -> analysis_song_structure_json payload).
rust::String get_song_structure_json(const Engine::HirariUnifiedEngine& engine) {
    auto* mutable_engine = const_cast<Engine::HirariUnifiedEngine*>(&engine);
    const auto sections = mutable_engine->run_structural_analysis();
    std::vector<uint8_t> kinds;
    std::vector<uint64_t> starts;
    std::vector<uint64_t> ends;
    std::vector<float> energies;
    std::vector<float> flows;
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

rust::Vec<float> get_runtime_telemetry(const BridgeFFI::AudioEngine& bridge) {
    rust::Vec<float> out;
    const auto* engine = &bridge.get_core();
    if (!engine) return out;
    // Same 10-element shape as the deleted AudioEngine::get_runtime_health_v(),
    // sourced from the engine's raw telemetry block so the UI snapshot keeps
    // its established index contract (0..9). The block publish is a thin
    // seqlock-style handoff in the audio callback; copy with retries.
    Engine::HirariUnifiedEngine::TelemetryData telemetry{};
    for (int attempt = 0; attempt < 8; ++attempt) {
        if (engine->copy_telemetry(engine->get_active_telemetry_idx(), telemetry)) {
            break;
        }
    }
    out.reserve(10);
    const auto peak = [](float const* samples, size_t n) -> float {
        float best = 0.0f;
        for (size_t i = 0; i < n; ++i) {
            if (std::isfinite(samples[i])) {
                const float abs = samples[i] < 0.0f ? -samples[i] : samples[i];
                if (abs > best) {
                    best = abs;
                }
            }
        }
        return best;
    };
    out.push_back(telemetry.dspLoad);
    out.push_back(peak(telemetry.peaksL, 256));
    out.push_back(peak(telemetry.peaksR, 256));
    out.push_back(telemetry.correlation);
    out.push_back(static_cast<float>(telemetry.activeVoices));
    out.push_back(static_cast<float>(telemetry.count));
    out.push_back(static_cast<float>(telemetry.version & 0x00ffffff));
    out.push_back(bridge.is_driver_running() ? 1.0f : 0.0f);
    out.push_back(engine->is_playing() ? 1.0f : 0.0f);
    out.push_back(static_cast<float>(engine->get_playhead()));
    return out;
}

} // namespace Hirari::Core::Bridge
