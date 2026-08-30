#pragma once

#include <vector>
#include <map>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>
#include <atomic>
#include <mutex>
#include <optional>

namespace Aura::Core::Engine {

/**
 * @class TempoMap
 * @brief Zero-Drift Master Clock for Professional DAW sessions.
 * Uses C++20 atomic shared pointers and trapezoidal integration
 * for tempo ramps to ensure sample-accurate beat alignment.
 *
 * Thread Safety:
 * - addTempo() / setBPM() are UI-thread operations (mutex-protected).
 * - samplesToBeats() / beatsToSamples() / getBPMAt() are RT-safe read-only
 *   operations on a lock-free shared_ptr snapshot.
 */
class TempoMap {
public:
    struct Event {
        uint64_t samplePos;
        double bpm;
        bool ramp;
        double worldBeats; // integrated beats from session start to this event
    };

    struct TimeSignatureEvent {
        uint64_t samplePos;
        uint8_t numerator;
        uint8_t denominator;
        double beat;
    };

    void addTimeSignature(double beat, uint8_t numerator, uint8_t denominator,
                          double sampleRate) {
        if (!std::isfinite(beat) || beat < 0.0 || numerator == 0 || numerator > 32 ||
            !std::isfinite(sampleRate) || sampleRate <= 0.0 ||
            !(denominator == 1 || denominator == 2 || denominator == 4 ||
              denominator == 8 || denominator == 16 || denominator == 32)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto next = std::make_shared<std::vector<TimeSignatureEvent>>(*m_signatures);
        const uint64_t sample = beatsToSamples(beat, sampleRate);
        auto it = std::lower_bound(next->begin(), next->end(), beat,
            [](const TimeSignatureEvent& event, double value) { return event.beat < value; });
        if (it != next->end() && it->beat == beat) {
            it->samplePos = sample;
            it->numerator = numerator;
            it->denominator = denominator;
        } else {
            next->insert(it, {sample, numerator, denominator, beat});
        }
        std::atomic_store(&m_signatures,
            std::const_pointer_cast<const std::vector<TimeSignatureEvent>>(next));
    }

    std::vector<TimeSignatureEvent> getTimeSignatures() const {
        auto signatures = std::atomic_load(&m_signatures);
        return signatures ? *signatures : std::vector<TimeSignatureEvent>{};
    }

    std::optional<TimeSignatureEvent> getTimeSignatureAt(double beat) const {
        if (!std::isfinite(beat) || beat < 0.0) return std::nullopt;
        auto signatures = std::atomic_load(&m_signatures);
        if (!signatures) return std::nullopt;
        const auto it = std::find_if(signatures->begin(), signatures->end(),
            [beat](const TimeSignatureEvent& event) {
                return std::fabs(event.beat - beat) < 1.0e-9;
            });
        return it == signatures->end() ? std::nullopt : std::optional<TimeSignatureEvent>(*it);
    }

    bool removeTimeSignature(double beat, double sampleRate) {
        if (!std::isfinite(beat) || beat <= 0.0 ||
            !std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto next = std::make_shared<std::vector<TimeSignatureEvent>>(*m_signatures);
        const auto it = std::find_if(next->begin(), next->end(),
            [beat](const TimeSignatureEvent& event) {
                return std::fabs(event.beat - beat) < 1.0e-9;
            });
        if (it == next->end()) return false;
        next->erase(it);
        std::atomic_store(&m_signatures,
            std::const_pointer_cast<const std::vector<TimeSignatureEvent>>(next));
        return true;
    }

    void clearTimeSignatures() {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto signatures = std::make_shared<const std::vector<TimeSignatureEvent>>(
            std::vector<TimeSignatureEvent>{{0, 4, 4, 0.0}});
        std::atomic_store(&m_signatures, std::move(signatures));
    }

    static TempoMap& getInstance() { static TempoMap i; return i; }

    void addTempo(uint64_t pos, double bpm, double sampleRate, bool ramp = false) {
        if (!std::isfinite(bpm) || bpm <= 0.0 || !std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_sampleRate = sampleRate;
        auto newEvents = std::make_shared<std::vector<Event>>(*m_events);

        auto it = std::lower_bound(newEvents->begin(), newEvents->end(), pos,
                                   [](const Event& e, uint64_t p) { return e.samplePos < p; });

        if (it != newEvents->end() && it->samplePos == pos) {
            it->bpm = bpm; it->ramp = ramp;
        } else {
            newEvents->insert(it, {pos, bpm, ramp, 0.0});
        }

        recalculateIntegratedTime(newEvents.get(), sampleRate);
        std::atomic_store(&m_events, std::const_pointer_cast<const std::vector<Event>>(newEvents));
    }

    void setBPM(double bpm) {
        const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
        addTempo(0, bpm, sampleRate > 0.0 ? sampleRate : 48000.0);
    }

    bool removeTempo(uint64_t pos, double sampleRate) {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || pos == 0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto newEvents = std::make_shared<std::vector<Event>>(*m_events);
        const auto it = std::find_if(newEvents->begin(), newEvents->end(),
                                     [pos](const Event& event) { return event.samplePos == pos; });
        if (it == newEvents->end()) return false;
        newEvents->erase(it);
        recalculateIntegratedTime(newEvents.get(), sampleRate);
        std::atomic_store(&m_events, std::const_pointer_cast<const std::vector<Event>>(newEvents));
        return true;
    }

    void clear(double sampleRate, double initialBpm = 120.0) {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 ||
            !std::isfinite(initialBpm) || initialBpm <= 0.0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_sampleRate.store(sampleRate, std::memory_order_relaxed);
        auto events = std::make_shared<std::vector<Event>>();
        events->push_back({0, initialBpm, false, 0.0});
        std::atomic_store(&m_events, std::const_pointer_cast<const std::vector<Event>>(events));
    }

    // Control/UI-thread snapshot. The returned copy keeps enumeration away
    // from the realtime conversion path.
    std::vector<Event> getEvents() const {
        auto events = std::atomic_load(&m_events);
        return events ? *events : std::vector<Event>{};
    }

    std::optional<Event> getEventAt(uint64_t pos) const {
        auto events = std::atomic_load(&m_events);
        if (!events) return std::nullopt;
        const auto it = std::find_if(events->begin(), events->end(),
                                     [pos](const Event& event) { return event.samplePos == pos; });
        return it == events->end() ? std::nullopt : std::optional<Event>(*it);
    }

    /**
     * @brief Sample -> Beat conversion with zero-drift trapezoidal integration.
     * RT-safe: only reads from atomic shared_ptr snapshot.
     */
    double samplesToBeats(uint64_t samples, double sampleRate) const {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return 0.0;
        auto events = std::atomic_load(&m_events);
        if (!events || events->empty()) {
            // No tempo events: assume 120 BPM
            return (static_cast<double>(samples) / sampleRate) * (120.0 / 60.0);
        }

        // Find the last event at or before `samples`
        const auto& evs = *events;
        if (samples < evs.front().samplePos) {
            const double seconds = static_cast<double>(samples) / sampleRate;
            return seconds * (evs.front().bpm / 60.0);
        }
        size_t idx = evs.size() - 1;
        for (size_t i = 0; i < evs.size(); ++i) {
            if (evs[i].samplePos > samples) { idx = (i == 0) ? 0 : i - 1; break; }
        }

        const Event& ev = evs[idx];
        double deltaSeconds = static_cast<double>(samples - ev.samplePos) / sampleRate;

        if (ev.ramp && idx + 1 < evs.size()) {
            // Trapezoidal integration over ramp segment
            const Event& next = evs[idx + 1];
            double segSeconds = static_cast<double>(next.samplePos - ev.samplePos) / sampleRate;
            if (segSeconds > 0.0) {
                const double t = std::clamp(deltaSeconds / segSeconds, 0.0, 1.0);
                const double deltaBpm = next.bpm - ev.bpm;
                const double integratedBpmSeconds =
                    ev.bpm * deltaSeconds + 0.5 * deltaBpm * segSeconds * t * t;
                return ev.worldBeats + integratedBpmSeconds / 60.0;
            }
        }

        return ev.worldBeats + deltaSeconds * (ev.bpm / 60.0);
    }

    /**
     * @brief Beat -> Sample conversion using binary search + Newton iteration.
     * RT-safe: only reads from atomic shared_ptr snapshot.
     */
    uint64_t beatsToSamples(double beats, double sampleRate) const {
        if (!std::isfinite(beats) || !std::isfinite(sampleRate) || sampleRate <= 0.0 || beats <= 0.0)
            return 0;
        auto events = std::atomic_load(&m_events);
        if (!events || events->empty()) {
            return static_cast<uint64_t>(beats * 60.0 / 120.0 * sampleRate);
        }

        const auto& evs = *events;

        // Find the event segment that contains `beats`
        size_t idx = evs.size() - 1;
        for (size_t i = 0; i < evs.size(); ++i) {
            if (evs[i].worldBeats > beats) { idx = (i == 0) ? 0 : i - 1; break; }
        }

        const Event& ev = evs[idx];
        double deltaBeats = beats - ev.worldBeats;

        if (ev.ramp && idx + 1 < evs.size()) {
            // Invert the exact linear-BPM integral with a bounded Newton solve.
            const Event& next = evs[idx + 1];
            double segSeconds = static_cast<double>(next.samplePos - ev.samplePos) / sampleRate;
            if (segSeconds <= 0.0 || next.worldBeats <= ev.worldBeats) return ev.samplePos;
            const double deltaBpm = next.bpm - ev.bpm;
            double deltaSeconds = std::clamp(deltaBeats * 60.0 / ev.bpm, 0.0, segSeconds);
            for (int iteration = 0; iteration < 5; ++iteration) {
                const double t = std::clamp(deltaSeconds / segSeconds, 0.0, 1.0);
                const double f = (ev.bpm * deltaSeconds +
                                  0.5 * deltaBpm * segSeconds * t * t) / 60.0 - deltaBeats;
                const double derivative = (ev.bpm + deltaBpm * t) / 60.0;
                if (std::abs(derivative) < 1.0e-12) break;
                deltaSeconds = std::clamp(deltaSeconds - f / derivative, 0.0, segSeconds);
            }
            return ev.samplePos + static_cast<uint64_t>(deltaSeconds * sampleRate);
        }

        double deltaSeconds = deltaBeats / (ev.bpm / 60.0);
        return ev.samplePos + static_cast<uint64_t>(deltaSeconds * sampleRate);
    }

    /**
     * @brief Returns the BPM at a given sample position.
     * RT-safe.
     */
    double getBPMAt(uint64_t pos) const {
        auto events = std::atomic_load(&m_events);
        if (!events || events->empty()) return 120.0;

        const auto& evs = *events;
        if (pos < evs.front().samplePos) return evs.front().bpm;
        for (size_t i = 0; i < evs.size(); ++i) {
            const auto& ev = evs[i];
            if (ev.samplePos > pos) break;
            if (ev.ramp && i + 1 < evs.size() && pos < evs[i + 1].samplePos) {
                const auto& next = evs[i + 1];
                const double span = static_cast<double>(next.samplePos - ev.samplePos);
                const double t = span > 0.0
                    ? std::clamp(static_cast<double>(pos - ev.samplePos) / span, 0.0, 1.0)
                    : 0.0;
                return ev.bpm + (next.bpm - ev.bpm) * t;
            }
            if (ev.samplePos <= pos) {
                if (i + 1 == evs.size() || pos >= evs[i + 1].samplePos) continue;
                return ev.bpm;
            }
        }
        return evs.back().bpm;
    }

public:
    TempoMap() {
        m_events = std::make_shared<const std::vector<Event>>();
        m_signatures = std::make_shared<const std::vector<TimeSignatureEvent>>(
            std::vector<TimeSignatureEvent>{{0, 4, 4, 0.0}});
        m_sampleRate.store(48000.0, std::memory_order_relaxed);
    }

private:

    static void recalculateIntegratedTime(std::vector<Event>* events, double sampleRate) {
        if (!events || !std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        double currentBeats = 0.0;
        uint64_t lastSamples = 0;
        double lastBpm = 120.0;
        bool lastRamp = false;

        for (size_t i = 0; i < events->size(); ++i) {
            auto& ev = (*events)[i];
            uint64_t step = ev.samplePos - lastSamples;
            if (step > 0) {
                double avgBpm = lastBpm;
                if (lastRamp) avgBpm = (lastBpm + ev.bpm) * 0.5; // trapezoidal
                currentBeats += (static_cast<double>(step) / sampleRate) * (avgBpm / 60.0);
            }
            ev.worldBeats = currentBeats;
            lastSamples = ev.samplePos;
            lastBpm = ev.bpm;
            lastRamp = ev.ramp;
        }
    }

    std::shared_ptr<const std::vector<Event>> m_events;
    std::shared_ptr<const std::vector<TimeSignatureEvent>> m_signatures;
    mutable std::mutex m_mutex;
    std::atomic<double> m_sampleRate{48000.0};
};

} // namespace Aura::Core::Engine
