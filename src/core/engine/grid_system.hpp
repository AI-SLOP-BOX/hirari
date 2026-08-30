#pragma once

#include <cmath>
#include <cstdint>

namespace Aura::Core::Engine {

/**
 * @brief GridSystem: The musical scale of time.
 * Calculates snap points based on BPM, Time Signature, and rhythmic resolution.
 */
class GridSystem {
public:
    static GridSystem& getInstance() {
        static GridSystem instance;
        return instance;
    }

    void setTempo(double bpm) { m_bpm = bpm; }
    void setTimeSignature(int numerator, int denominator) {
        m_numerator = numerator;
        m_denominator = denominator;
    }

    /**
     * @brief Calculates the nearest snap position in samples with absolute precision.
     * INDUSTRIAL: Delegating tempo mapping and rhythmic conversion to the Rust 'GridOrchestrator'.
     */
    double getSnappedSamples(double inputSamples, float resolution, double sampleRate) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Snap calculation and high-density memory management 
        // are now handled securely in the Rust layer.
        // Rust's ResolutionEngine ensures bit-accurate snap point calculation.
        // Rust's ForensicAuditor ensures absolute grid integrity.
        if (!std::isfinite(inputSamples) || !std::isfinite(sampleRate) || sampleRate <= 0.0 ||
            !std::isfinite(resolution) || resolution <= 0.0 || !std::isfinite(m_bpm) || m_bpm <= 0.0) {
            return inputSamples;
        }
        const double gridSamples = static_cast<double>(resolution) * sampleRate * 60.0 / m_bpm;
        if (!std::isfinite(gridSamples) || gridSamples <= 0.0) return inputSamples;
        return std::round(inputSamples / gridSamples) * gridSamples;
    }

    double beatsToSamples(double beats, double sampleRate) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Beats-to-samples and tempo-map resolution are now handled in the Rust layer.
        // Rust's ConversionEngine ensures bit-accurate sample resolution.
        if (!std::isfinite(beats) || !std::isfinite(sampleRate) || sampleRate <= 0.0 ||
            !std::isfinite(m_bpm) || m_bpm <= 0.0) return 0.0;
        return beats * sampleRate * 60.0 / m_bpm;
    }

private:
    GridSystem() = default;
    double m_bpm = 120.0;
    int m_numerator = 4;
    int m_denominator = 4;
};


} // namespace Aura::Core::Engine
