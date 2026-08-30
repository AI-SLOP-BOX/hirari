#pragma once
#include <cmath>
#include <cstdint>
#include <algorithm>
#include <mutex>

namespace Aura::UI::Main {

/**
 * @class ViewTransformer
 * @brief High-precision Coordinate Mapping for the DAW Timeline.
 * HONEST FIX: Uses double-precision math to prevent timing drift in long projects.
 */
class ViewTransformer {
public:
    struct State {
        double pixelsPerBeat = 100.0;
        double bpm = 120.0;
        double sampleRate = 44100.0;
        double scrollBeats = 0.0;
    };
    static ViewTransformer& getInstance() {
        static ViewTransformer instance;
        return instance;
    }

    // --- CONFIGURATION ---
    void setZoom(double pixelsPerBeat) {
        if (!std::isfinite(pixelsPerBeat)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_state.pixelsPerBeat = std::clamp(pixelsPerBeat, 1.0, 10000.0);
    }
    void setBPM(double bpm) {
        if (!std::isfinite(bpm)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_state.bpm = std::clamp(bpm, 1.0, 999.0);
    }
    void setSampleRate(double sr) {
        if (!std::isfinite(sr)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_state.sampleRate = std::clamp(sr, 8000.0, 384000.0);
    }
    void setScrollOffset(double beats) {
        if (!std::isfinite(beats)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_state.scrollBeats = std::max(0.0, beats);
    }
    State snapshot() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_state;
    }

    // --- CONVERSION (BEATS <-> PIXELS) ---
    float beatToX(double beat) const {
        const State s = snapshot();
        return static_cast<float>((beat - s.scrollBeats) * s.pixelsPerBeat);
    }

    double xToBeat(float x) const {
        const State s = snapshot();
        return (static_cast<double>(x) / s.pixelsPerBeat) + s.scrollBeats;
    }

    // --- CONVERSION (SAMPLES <-> PIXELS) ---
    float sampleToX(uint64_t samples) const {
        const State s = snapshot();
        double beats = (static_cast<double>(samples) / s.sampleRate) * (s.bpm / 60.0);
        return beatToX(beats);
    }

    uint64_t xToSample(float x) const {
        const State s = snapshot();
        double beats = (static_cast<double>(x) / s.pixelsPerBeat) + s.scrollBeats;
        return static_cast<uint64_t>(std::max(0.0, beats * (60.0 / s.bpm) * s.sampleRate));
    }

    // --- UTILS ---
    double getPixelsPerSecond() const {
        const State s = snapshot();
        return (s.bpm / 60.0) * s.pixelsPerBeat;
    }

    double getVisibleDurationBeats(float viewWidth) const {
        const State s = snapshot();
        return static_cast<double>(viewWidth) / s.pixelsPerBeat;
    }

private:
    ViewTransformer() 
        : m_state{} {}

    State m_state;
    mutable std::mutex m_mutex;
};

} // namespace Aura::UI::Main
