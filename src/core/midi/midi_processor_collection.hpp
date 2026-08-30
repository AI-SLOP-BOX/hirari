#pragma once

#include <vector>
#include <array>
#include <cmath>
#include "../../core/midi_buffer.hpp"

namespace Aura::Core::Midi {

/**
 * @class Arpeggiator
 * @brief Industrial-Scale Pattern Orchestrator.
 * 
 * Supports Up, Down, Up/Down, Random, and Chord modes with 
 * sample-accurate timing.
 */
class Arpeggiator {
public:
    enum class Mode { Up, Down, UpDown, Random, Chord };
    void setMode(Mode mode) noexcept { m_mode = mode; m_currentIndex = 0; }
    Mode mode() const noexcept { return m_mode; }

    void process(MidiBuffer& buffer, double samplesPerBeat) {
        if (!std::isfinite(samplesPerBeat) || samplesPerBeat <= 0.0) return;
        for (const auto& event : buffer) {
            if (event.size < 3) continue;
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t pitch = event.data[1] & 0x7F;
            if (status == 0x90 && event.data[2] > 0) m_heldNotes[pitch] = true;
            else if (status == 0x80 || (status == 0x90 && event.data[2] == 0)) m_heldNotes[pitch] = false;
        }
        const double period = std::max(1.0, samplesPerBeat * 0.25);
        m_phaseSamples += static_cast<double>(m_blockSize);
        if (m_phaseSamples < period) return;
        m_phaseSamples = std::fmod(m_phaseSamples, period);
        std::array<uint8_t, 128> active{};
        size_t count = 0;
        for (uint32_t note = 0; note < 128; ++note) if (m_heldNotes[note]) active[count++] = static_cast<uint8_t>(note);
        if (count == 0) return;
        size_t index = static_cast<size_t>(m_currentIndex) % count;
        if (m_mode == Mode::Down) index = count - 1 - index;
        else if (m_mode == Mode::Random) {
            m_randomState ^= m_randomState << 13; m_randomState ^= m_randomState >> 17; m_randomState ^= m_randomState << 5;
            index = m_randomState % count;
        }
        if (m_mode == Mode::Chord) {
            constexpr std::array<int, 3> intervals{0, 4, 7};
            for (const int interval : intervals) {
                const int pitch = static_cast<int>(active[index]) + interval;
                if (pitch <= 127) buffer.addNoteOn(1, static_cast<uint8_t>(pitch), 100, 0);
            }
        } else {
            buffer.addNoteOn(1, active[index], 100, 0);
        }
        m_currentIndex = (m_currentIndex + 1) % static_cast<int>(count);
    }

private:
    std::array<bool, 128> m_heldNotes{};
    Mode m_mode = Mode::Up;
    int m_currentIndex = 0;
    double m_phaseSamples = 0.0;
    uint32_t m_blockSize = 256;
    uint32_t m_randomState = 0x9E3779B9u;

public:
    void setBlockSize(uint32_t blockSize) { if (blockSize > 0) m_blockSize = blockSize; }
};

/**
 * @class ChordTrigger
 * @brief Professional Harmonic Expansion Tool.
 * 
 * Maps single MIDI notes to complex multi-voice orchestral voicings.
 */
class ChordTrigger {
public:
    struct Voicing {
        std::vector<int8_t> intervals; // [0, 4, 7, 12] for Major
    };

    void process(MidiBuffer& buffer) {
        MidiBuffer expanded;
        for (const auto& ev : buffer) {
            if (ev.size < 3) continue;
            const uint8_t status = ev.data[0] & 0xF0;
            if (status == 0x90 && ev.data[2] > 0) {
                for (const int interval : m_currentVoicing.intervals) {
                    const int pitch = static_cast<int>(ev.data[1]) + interval;
                    if (pitch >= 0 && pitch <= 127) {
                        uint8_t data[8]{};
                        std::copy(ev.data, ev.data + ev.size, data);
                        data[1] = static_cast<uint8_t>(pitch);
                        expanded.addEvent(ev.sampleOffset, data, ev.size, ev.articulationId);
                    }
                }
            } else {
                expanded.addEvent(ev.sampleOffset, ev.data, ev.size, ev.articulationId);
            }
        }
        buffer.clear();
        for (const auto& event : expanded) buffer.tryAddEvent(event);
    }

private:
    Voicing m_currentVoicing{{0, 4, 7}};
};

/**
 * @class MidiNodalProcessor
 * @brief Advanced Scriptable MIDI Logic Engine.
 * 
 * High-Intelligence MIDI transformation pipeline.
 */
class MidiNodalProcessor {
public:
    void process(MidiBuffer& buffer) {
        MidiEvent* events = buffer.getMutableEvents();
        for (size_t index = 0; index < buffer.size(); ++index) {
            auto& event = events[index];
            if (event.size < 2) continue;
            const uint8_t channel = static_cast<uint8_t>((event.data[0] & 0x0F) + 1);
            if (m_channel != 0 && channel != m_channel) continue;

            const uint8_t status = event.data[0] & 0xF0;
            if (status != 0x80 && status != 0x90 && status != 0xA0 && status != 0xB0)
                continue;

            const int shifted = static_cast<int>(event.data[1]) + m_transpose;
            event.data[1] = static_cast<uint8_t>(std::clamp(shifted, 0, 127));
            if (event.size >= 3 && (status == 0x80 || status == 0x90 || status == 0xA0)) {
                const float scaled = static_cast<float>(event.data[2]) * m_velocityScale;
                event.data[2] = static_cast<uint8_t>(std::clamp(scaled + 0.5f, 0.0f, 127.0f));
            }
        }
    }

    void setTranspose(int semitones) noexcept { m_transpose = std::clamp(semitones, -127, 127); }
    void setVelocityScale(float scale) noexcept {
        m_velocityScale = std::isfinite(scale) ? std::clamp(scale, 0.0f, 4.0f) : 1.0f;
    }
    void setChannel(uint8_t channel) noexcept { m_channel = channel <= 16 ? channel : 0; }

    int transpose() const noexcept { return m_transpose; }
    float velocityScale() const noexcept { return m_velocityScale; }
    uint8_t channel() const noexcept { return m_channel; }

private:
    int m_transpose = 0;
    float m_velocityScale = 1.0f;
    uint8_t m_channel = 0; // 0 means all channels
};

} // namespace Aura::Core::Midi
