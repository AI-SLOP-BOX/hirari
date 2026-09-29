#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <type_traits>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade only. Rust owns the immutable tempo/signature
// snapshots, writer serialization, mutation, and sample/beat calculations.
// ArcSwap-backed Rust reads keep the transport conversion path free of locks.
class TempoMap {
public:
    struct Event {
        uint64_t samplePos;
        double bpm;
        bool ramp;
        double worldBeats;
    };
    static_assert(std::is_standard_layout_v<Event>);
    static_assert(offsetof(Event, samplePos) == 0 && offsetof(Event, bpm) == 8 &&
                  offsetof(Event, ramp) == 16 && offsetof(Event, worldBeats) == 24 &&
                  sizeof(Event) == 32);

    struct TimeSignatureEvent {
        uint64_t samplePos;
        uint8_t numerator;
        uint8_t denominator;
        double beat;
    };
    static_assert(std::is_standard_layout_v<TimeSignatureEvent>);
    static_assert(offsetof(TimeSignatureEvent, samplePos) == 0 &&
                  offsetof(TimeSignatureEvent, numerator) == 8 &&
                  offsetof(TimeSignatureEvent, denominator) == 9 &&
                  offsetof(TimeSignatureEvent, beat) == 16 && sizeof(TimeSignatureEvent) == 24);

    TempoMap() : m_state(hirari_tempo_map_create()) {}
    ~TempoMap() { hirari_tempo_map_destroy(m_state); }
    TempoMap(const TempoMap&) = delete;
    TempoMap& operator=(const TempoMap&) = delete;

    void addTimeSignature(double beat, uint8_t numerator, uint8_t denominator,
                          double sampleRate) {
        (void)hirari_tempo_map_add_time_signature(
            m_state, beat, numerator, denominator, sampleRate);
    }

    std::vector<TimeSignatureEvent> getTimeSignatures() const {
        return readSnapshot<TimeSignatureEvent,
                            hirari_tempo_map_get_signature_count,
                            hirari_tempo_map_copy_signatures>(m_state);
    }

    std::optional<TimeSignatureEvent> getTimeSignatureAt(double beat) const {
        TimeSignatureEvent event{};
        return hirari_tempo_map_get_signature_at(m_state, beat, &event)
            ? std::optional<TimeSignatureEvent>(event)
            : std::nullopt;
    }

    bool removeTimeSignature(double beat, double sampleRate) {
        return hirari_tempo_map_remove_time_signature(m_state, beat, sampleRate);
    }

    void clearTimeSignatures() {
        (void)hirari_tempo_map_clear_time_signatures(m_state);
    }

    static TempoMap& getInstance() {
        static TempoMap instance;
        return instance;
    }

    void addTempo(uint64_t pos, double bpm, double sampleRate, bool ramp = false) {
        (void)hirari_tempo_map_add_tempo(m_state, pos, bpm, sampleRate, ramp);
    }

    bool replaceEventsAtBeats(std::vector<Event> events, double sampleRate) {
        return !events.empty() && hirari_tempo_map_replace_events_at_beats(
            m_state, events.data(), events.size(), sampleRate);
    }

    void setBPM(double bpm) {
        (void)hirari_tempo_map_set_bpm(m_state, bpm);
    }

    float currentBpm() const {
        return hirari_tempo_map_get_current_bpm(m_state);
    }

    void setCurrentBpm(float bpm) {
        hirari_tempo_map_set_current_bpm(m_state, bpm);
    }

    bool removeTempo(uint64_t pos, double sampleRate) {
        return hirari_tempo_map_remove_tempo(m_state, pos, sampleRate);
    }

    void clear(double sampleRate, double initialBpm = 120.0) {
        (void)hirari_tempo_map_clear(m_state, sampleRate, initialBpm);
    }

    std::vector<Event> getEvents() const {
        return readSnapshot<Event, hirari_tempo_map_get_event_count,
                            hirari_tempo_map_copy_events>(m_state);
    }

    std::optional<Event> getEventAt(uint64_t pos) const {
        Event event{};
        return hirari_tempo_map_get_event_at(m_state, pos, &event)
            ? std::optional<Event>(event)
            : std::nullopt;
    }

    double samplesToBeats(uint64_t samples, double sampleRate) const {
        return hirari_tempo_map_samples_to_beats(m_state, samples, sampleRate);
    }

    uint64_t beatsToSamples(double beats, double sampleRate) const {
        return hirari_tempo_map_beats_to_samples(m_state, beats, sampleRate);
    }

    double getBPMAt(uint64_t pos) const {
        return hirari_tempo_map_bpm_at_sample(m_state, pos);
    }

private:
    template <typename EventType,
              size_t (*Count)(const void*),
              size_t (*Copy)(const void*, void*, size_t)>
    static std::vector<EventType> readSnapshot(const void* state) {
        std::vector<EventType> result(Count(state));
        while (true) {
            const size_t count = Copy(state, result.empty() ? nullptr : result.data(), result.size());
            if (count <= result.size()) {
                result.resize(count);
                return result;
            }
            result.resize(count);
        }
    }

    void* m_state;
};

} // namespace Hirari::Core::Engine
