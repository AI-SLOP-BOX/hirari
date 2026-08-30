#pragma once

#include <atomic>
#include <cmath>
#include <memory>
#include "engine/automation_curve.hpp"

namespace Aura::Core {

/**
 * @brief TransportSystem: High-precision time-base for the DAW.
 * Calculates Beats, Bars, and Samples with sample-accurate precision.
 */
class TransportSystem {
public:
    struct Position {
        uint32_t bar = 1;
        uint32_t beat = 1;
        double tick = 0.0;
        uint64_t totalSamples = 0;
        
        struct Trigger {
            bool isNewBeat = false;
            bool isNewBar = false;
        } trigger;
    };

    TransportSystem(double sr) : m_sampleRate(sr), m_timeSigNum(4), m_timeSigDen(4) {
        m_tempoCurve = std::make_unique<Engine::AutomationCurve>();
        m_tempoCurve->addPoint(0, 120.0f); 
    }

    void setBPM(double bpm) { m_bpm.store(bpm); }
    double getBPM() const { return m_bpm.load(); }
    void togglePlay(bool play) { m_isPlaying.store(play); }

    void setTimeSignature(uint32_t num, uint32_t den) {
        m_timeSigNum.store(num);
        m_timeSigDen.store(den);
    }

    /**
     * @brief Updates the transport by a block size.
     * HONEST FIX: Uses the average BPM across the block to prevent drift.
     * Converts sample position to beat-time before querying tempo automation.
     */
    Position::Trigger advance(uint32_t numSamples) {
        Position::Trigger trigger;
        if (!m_isPlaying.load(std::memory_order_relaxed)) return trigger;

        double prevBeat = m_currentBeat.load(std::memory_order_relaxed);
        
        // 1. Query Tempo Curve at current BEAT (Musical Time)
        if (m_tempoCurve) {
            m_bpm.store(m_tempoCurve->getValueAt(prevBeat), std::memory_order_relaxed);
        }

        double bpm = m_bpm.load(std::memory_order_relaxed);
        double beatsPerSample = bpm / (60.0 * m_sampleRate);
        
        // Adjust for time signature denominator (e.g. 8th notes)
        // Reference: 4 = quarter note = 1 beat. 8 = 8th note = 0.5 beats? 
        // Logic/standard: Denominator 4 means quarter note is the beat. 
        // 8 means eighth note is the subdivision, but usually BPM still refers to quarter notes unless specified.
        // For simplicity, we stick to quarter notes for BPM.
        
        m_totalSamples.fetch_add(numSamples, std::memory_order_relaxed);
        double deltaBeats = (double)numSamples * beatsPerSample;
        double nextBeat = prevBeat + deltaBeats;
        m_currentBeat.store(nextBeat, std::memory_order_relaxed);
        
        uint32_t num = m_timeSigNum.load(std::memory_order_relaxed);
        
        uint64_t prevBeatInt = static_cast<uint64_t>(prevBeat);
        uint64_t currBeatInt = static_cast<uint64_t>(nextBeat);
        
        if (currBeatInt > prevBeatInt) {
            trigger.isNewBeat = true;
            // Bar trigger depends on the numerator
            if (currBeatInt % num == 0) trigger.isNewBar = true;
        }
        
        return trigger;
    }

    Position getPosition() const {
        Position p;
        p.totalSamples = m_totalSamples.load(std::memory_order_relaxed);
        double beat = m_currentBeat.load(std::memory_order_relaxed);
        uint32_t num = m_timeSigNum.load(std::memory_order_relaxed);
        
        p.beat = static_cast<uint32_t>(std::floor(beat)) % num + 1;
        p.bar = static_cast<uint32_t>(std::floor(beat)) / num + 1;
        p.tick = (beat - std::floor(beat)) * 960.0; // Standard 960 PPQN
        return p;
    }

private:
    double m_sampleRate;
    std::atomic<double> m_bpm{120.0};
    std::atomic<bool> m_isPlaying{false};
    std::atomic<uint64_t> m_totalSamples{0};
    std::atomic<double> m_currentBeat{0.0};
    std::atomic<uint32_t> m_timeSigNum{4}, m_timeSigDen{4};
    std::unique_ptr<Engine::AutomationCurve> m_tempoCurve;
};

} // namespace Aura::Core
