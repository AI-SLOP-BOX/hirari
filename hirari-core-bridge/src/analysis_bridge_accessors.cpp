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
rust::Vec<float> get_spectrum(const Engine::HirariUnifiedEngine* engine, uint32_t idx) {
    ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData snapshot{};
    if (!engine->copy_telemetry(idx, snapshot)) return {};
    rust::Vec<float> result;
    result.reserve(512);
    for (uint32_t i = 0; i < 512; ++i) result.push_back(snapshot.spectrum[i]);
    return result;
}

// Current meter (true peak, correlation, integrated/short-term loudness).
BridgeLoudness get_master_meter_data(const Engine::HirariUnifiedEngine* engine) {
    const auto meter = engine->getMasterMeterData();
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
rust::String get_intelligence_dashboard_json(const Engine::HirariUnifiedEngine* engine) {
    if (!engine) return {};
    const auto meter = engine->getMasterMeterData();
    return ::Hirari::Core::Bridge::analysis_dashboard_json(
        meter.lufsShortTerm, meter.lufsIntegrated,
        meter.truePeakL, meter.truePeakR, meter.correlation);
}

// Song structure JSON (sections -> analysis_song_structure_json payload).
rust::String get_song_structure_json(const Engine::HirariUnifiedEngine* engine) {
    if (!engine) return rust::String("{\"available\":false,\"sections\":[]}");
    auto* mutable_engine = const_cast<Engine::HirariUnifiedEngine*>(engine);
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

} // namespace Hirari::Core::Bridge
