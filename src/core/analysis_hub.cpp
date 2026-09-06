#include "analysis_hub.hpp"
#include "aura_unified_engine.hpp"
#include "neural_bridge.hpp"
#include "engine/track.hpp"
#include "engine/engine_analyzer.hpp"
#include <algorithm>
#include <cmath>
#include <sstream>

namespace Aura::Core::BridgeFFI {

    rust::Vec<float> AnalysisHub::get_mixing_advice_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;

        auto& oracle = ::Aura::Core::Engine::EngineAnalyzer::getInstance();
        oracle.updateAdvice({}, m_engine->getMasterMeterData());
        ::Aura::Core::Engine::EngineAnalyzer::Advice advice[16];
        size_t count = oracle.getAdvice(advice, 16);
        result.reserve(count * 3);

        for (size_t i = 0; i < count; ++i) {
            result.push_back((float)advice[i].id);
            result.push_back((float)advice[i].severity);
            result.push_back(0.0f); // Meta
        }
        return result;
    }

    rust::String AnalysisHub::get_arrangement_advice(uint32_t) const {
        auto& oracle = ::Aura::Core::Engine::EngineAnalyzer::getInstance();
        return rust::String(oracle.getArrangementAdvice());
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
        result.reserve(512);
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        
        const double sampleRate = std::clamp(m_engine->get_sample_rate(), 8000.0, 384000.0);
        const double nyquist = sampleRate * 0.5;
        for (size_t i = 0; i < 512; ++i) {
            // Log-frequency display mapping with linear interpolation between
            // FFT bins, avoiding the low-frequency pile-up of integer lookup.
            const double normalized = static_cast<double>(i) / 511.0;
            const double frequency = 20.0 * std::pow(nyquist / 20.0, normalized);
            const double bin = frequency / nyquist * 511.0;
            const size_t lo = std::min<size_t>(static_cast<size_t>(bin), 511);
            const size_t hi = std::min<size_t>(lo + 1, 511);
            const float a = std::isfinite(telemetry.spectrum[lo]) ? telemetry.spectrum[lo] : 0.0f;
            const float b = std::isfinite(telemetry.spectrum[hi]) ? telemetry.spectrum[hi] : a;
            result.push_back(std::clamp(a + (b - a) * static_cast<float>(bin - lo), 0.0f, 1.0f));
        }
        return result;
    }

    rust::Vec<float> AnalysisHub::get_mixer_levels_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        auto l = m_engine->get_track_peaks_l();
        auto r = m_engine->get_track_peaks_r();
        
        const size_t count = std::min(l.size(), r.size());
        result.reserve(count * 2);
        for(size_t i=0; i < count; ++i) {
             result.push_back(l[i]);
             result.push_back(r[i]);
        }
        return result;
    }

    std::vector<::Aura::Core::Bridge::PlainClash> AnalysisHub::get_spectral_clash_v() const {
        std::vector<::Aura::Core::Bridge::PlainClash> result;
        if (!m_engine) return result;

        auto& oracle = ::Aura::Core::Engine::EngineAnalyzer::getInstance();
        oracle.updateAdvice({}, m_engine->getMasterMeterData());
        ::Aura::Core::Engine::EngineAnalyzer::Advice advice[16];
        size_t count = oracle.getAdvice(advice, 16);

        for (size_t i = 0; i < count; ++i) {
            if (advice[i].id >= 1000) {
                 result.push_back({static_cast<uint32_t>(advice[i].id), static_cast<float>(advice[i].severity), 0u});
            }
        }
        return result;
    }

    ::Aura::Core::Bridge::PlainLoudness AnalysisHub::get_master_loudness_v() const {
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
        result.reserve(128);
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        for (size_t i = 0; i < 128; ++i) {
            const size_t bin = std::min<size_t>(i * 4, 511);
            const float current = telemetry.spectrum[bin];
            const float next = telemetry.spectrum[std::min<size_t>(bin + 1, 511)];
            result.push_back(std::clamp(next - current, -1.0f, 1.0f));
        }
        return result;
    }

    rust::String AnalysisHub::get_intelligence_dashboard_json() const { 
        char buf[384];
        if (!m_engine) return rust::String("{\"available\":false}");
        const auto meter = m_engine->getMasterMeterData();
        const float health = std::clamp(1.0f - std::max(std::fabs(static_cast<float>(meter.truePeakL)),
                                                        std::fabs(static_cast<float>(meter.truePeakR))) * 0.1f,
                                       0.0f, 1.0f);
        std::snprintf(buf, sizeof(buf),
                      "{\"available\":true,\"health\":%.3f,\"lufs_short\":%.2f,\"lufs_integrated\":%.2f,\"true_peak_l\":%.2f,\"true_peak_r\":%.2f,\"correlation\":%.3f}",
                      health, static_cast<float>(meter.lufsShortTerm), static_cast<float>(meter.lufsIntegrated),
                      static_cast<float>(meter.truePeakL), static_cast<float>(meter.truePeakR),
                      static_cast<float>(meter.correlation));
        return rust::String(buf);
    }

    void AnalysisHub::trigger_background_analysis() const {
        if (!m_engine) return;

        // Keep this call real-time friendly: the bridge only enqueues a compact
        // advice packet in its lock-free queue; expensive analysis stays off the
        // caller's (potentially audio) thread.
        const auto meter = m_engine->getMasterMeterData();
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        const float truePeak = std::max(static_cast<float>(meter.truePeakL),
                                        static_cast<float>(meter.truePeakR));
        ::Aura::Core::AI::NeuralBridge::getInstance().evaluateSignal(
            static_cast<float>(meter.lufsIntegrated), truePeak, telemetry.spectrum);
    }

    rust::String AnalysisHub::get_song_structure_json() const {
        if (!m_engine) return rust::String("{\"available\":false,\"sections\":[]}");
        const auto sections = m_engine->run_structural_analysis();
        std::ostringstream json;
        json << "{\"available\":true,\"sections\":[";
        for (size_t i = 0; i < sections.size(); ++i) {
            const auto& section = sections[i];
            if (i > 0) json << ',';
            const char* name = "Unknown";
            switch (section.type) {
                case ::Aura::Core::Composition::NeuralArrangementKernel::SectionType::Intro: name = "Intro"; break;
                case ::Aura::Core::Composition::NeuralArrangementKernel::SectionType::Verse: name = "Verse"; break;
                case ::Aura::Core::Composition::NeuralArrangementKernel::SectionType::Chorus: name = "Chorus"; break;
                case ::Aura::Core::Composition::NeuralArrangementKernel::SectionType::Bridge: name = "Bridge"; break;
                case ::Aura::Core::Composition::NeuralArrangementKernel::SectionType::Outro: name = "Outro"; break;
                default: break;
            }
            json << "{\"name\":\"" << name << "\",\"start_sample\":" << section.startSample
                 << ",\"end_sample\":" << section.endSample
                 << ",\"energy\":" << section.energyLevel
                 << ",\"flow\":" << section.narrativeFlowScore << '}';
        }
        json << "]}";
        return rust::String(json.str());
    }

    rust::Vec<float> AnalysisHub::get_spectral_partials_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        result.reserve(128);
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        for (size_t i = 1; i + 1 < 512; ++i) {
            const float value = telemetry.spectrum[i];
            if (value >= telemetry.spectrum[i - 1] && value >= telemetry.spectrum[i + 1] && value > 0.01f) {
                result.push_back(static_cast<float>(i));
                result.push_back(value);
            }
        }
        return result;
    }

    rust::String AnalysisHub::get_creative_advice() const {
        if (!m_engine) return rust::String("Analysis unavailable.");
        const auto meter = m_engine->getMasterMeterData();
        if (meter.truePeakL > -0.1 || meter.truePeakR > -0.1)
            return rust::String("Reduce master gain: true peak is close to clipping.");
        if (meter.correlation < 0.0)
            return rust::String("Check stereo phase: the master correlation is negative.");
        return rust::String("Master headroom and stereo correlation are currently healthy.");
    }

    rust::Vec<float> AnalysisHub::get_mel_spectrogram_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        result.reserve(128);
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        for (size_t band = 0; band < 128; ++band) {
            const float center = static_cast<float>(band) / 127.0f * 511.0f;
            const float width = std::max(1.0f, 511.0f / 127.0f);
            const int first = std::max(0, static_cast<int>(std::floor(center - width)));
            const int last = std::min(511, static_cast<int>(std::ceil(center + width)));
            float weighted = 0.0f, weightSum = 0.0f;
            for (int bin = first; bin <= last; ++bin) {
                const float weight = std::max(0.0f, 1.0f - std::fabs(static_cast<float>(bin) - center) / width);
                const float value = std::isfinite(telemetry.spectrum[bin]) ? telemetry.spectrum[bin] : 0.0f;
                weighted += std::max(0.0f, value) * weight;
                weightSum += weight;
            }
            result.push_back(weightSum > 0.0f ? std::clamp(weighted / weightSum, 0.0f, 1.0f) : 0.0f);
        }
        return result;
    }

    float AnalysisHub::get_phase_correlation() const {
        if (!m_engine) return 0.0f;
        return std::clamp(static_cast<float>(m_engine->getMasterMeterData().correlation), -1.0f, 1.0f);
    }

    rust::Vec<float> AnalysisHub::get_motion_vectors_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        result.reserve(128 * 2);
        // Encode spectral motion as a compact vector field: x is signed local
        // spectral slope, y is positive onset/energy movement. This keeps the
        // UI independent of the analyzer's internal FFT size.
        for (size_t band = 0; band < 128; ++band) {
            const size_t bin = std::min<size_t>(band * 4, 511);
            const size_t next = std::min<size_t>(bin + 3, 511);
            const float left = std::isfinite(telemetry.spectrum[bin]) ? telemetry.spectrum[bin] : 0.0f;
            const float right = std::isfinite(telemetry.spectrum[next]) ? telemetry.spectrum[next] : 0.0f;
            const float slope = std::clamp(right - left, -1.0f, 1.0f);
            const float magnitude = std::clamp(0.5f * (std::fabs(left) + std::fabs(right)), 0.0f, 1.0f);
            result.push_back(slope * (0.25f + 0.75f * magnitude));
            result.push_back(magnitude);
        }
        return result;
    }

    float AnalysisHub::get_motion_energy() const {
        if (!m_engine) return 0.0f;
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        float energy = 0.0f;
        for (size_t i = 1; i < 512; ++i) energy += std::fabs(telemetry.spectrum[i] - telemetry.spectrum[i - 1]);
        return energy / 511.0f;
    }

    rust::Vec<float> AnalysisHub::get_synesthesia_colors_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        const auto& telemetry = m_engine->get_telemetry(m_engine->get_active_telemetry_idx());
        result.reserve(128 * 3);
        // Map each spectral band to a stable RGB triplet. Hue follows pitch
        // class while brightness follows measured band energy.
        for (size_t band = 0; band < 128; ++band) {
            const size_t bin = std::min<size_t>(band * 4, 511);
            const float raw = std::isfinite(telemetry.spectrum[bin]) ? telemetry.spectrum[bin] : 0.0f;
            const float value = std::clamp(std::fabs(raw), 0.0f, 1.0f);
            const float hue = static_cast<float>(band % 12) / 12.0f;
            const float h6 = hue * 6.0f;
            const int sector = static_cast<int>(h6) % 6;
            const float f = h6 - std::floor(h6);
            const float q = value * (1.0f - 0.65f * f);
            const float t = value * (1.0f - 0.65f * (1.0f - f));
            float r = 0.0f, g = 0.0f, b = 0.0f;
            switch (sector) {
                case 0: r = value; g = t; break;
                case 1: r = q; g = value; break;
                case 2: g = value; b = t; break;
                case 3: g = q; b = value; break;
                case 4: r = t; b = value; break;
                default: r = value; b = q; break;
            }
            result.push_back(r); result.push_back(g); result.push_back(b);
        }
        return result;
    }

} // namespace Aura::Core::BridgeFFI
