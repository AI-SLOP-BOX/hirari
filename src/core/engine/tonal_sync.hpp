#pragma once

#include <stdint.h>
#include <atomic>
#include <vector>
#include <map>

namespace Aura::Core::Engine {

/**
 * @class TonalSync
 * @brief Synchronizes global tonality (Scale/Root) across all tracks.
 */
class TonalSync {
public:
    enum class ScaleType { 
        Major, Minor, HarmonicMinor, MelodicMinor, 
        Dorian, Phrygian, Lydian, Mixolydian, Aeolian, Locrian,
        PentatonicMajor, PentatonicMinor 
    };

    static TonalSync& getInstance() {
        static TonalSync instance;
        return instance;
    }

    /**
     * @brief SCALE: Sets the project's global tonality with industrial precision and theory sovereignty.
     * INDUSTRIAL: Delegating scale management and harmonic resolution to the Rust 'TonalOrchestrator'.
     */
    /**
     * @brief SCALE: Sets the project's global tonality with industrial precision and theory sovereignty.
     * INDUSTRIAL: Delegating scale management and harmonic resolution to the Rust 'TonalOrchestrator'.
     */
    void setScale(int32_t root, ScaleType type) {
        m_root.store(((root % 12) + 12) % 12, std::memory_order_release);
        m_pattern.store(patternFor(type), std::memory_order_release);
        m_scaleType.store(static_cast<uint32_t>(type), std::memory_order_release);
    }

    bool isNoteInScale(int32_t midiNote) const {
        if (midiNote < 0 || midiNote > 127) return false;
        const int relative = ((midiNote % 12) - m_root.load(std::memory_order_acquire) + 12) % 12;
        return (m_pattern.load(std::memory_order_acquire) & (1u << relative)) != 0;
    }

    int32_t root() const { return m_root.load(std::memory_order_acquire); }
    uint32_t scaleType() const { return m_scaleType.load(std::memory_order_acquire); }

public:
    static uint32_t patternFor(ScaleType type) {
        switch (type) {
            case ScaleType::Major:
                return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 11);
            case ScaleType::Minor:
            case ScaleType::Aeolian:
                return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 8) | (1u << 10);
            case ScaleType::HarmonicMinor:
                return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 8) | (1u << 11);
            case ScaleType::MelodicMinor:
                return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 11);
            case ScaleType::Dorian:
                return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 10);
            case ScaleType::Phrygian:
                return (1u << 0) | (1u << 1) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 8) | (1u << 10);
            case ScaleType::Lydian:
                return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 6) | (1u << 7) | (1u << 9) | (1u << 11);
            case ScaleType::Mixolydian:
                return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 10);
            case ScaleType::Locrian:
                return (1u << 0) | (1u << 1) | (1u << 3) | (1u << 5) | (1u << 6) | (1u << 8) | (1u << 10);
            case ScaleType::PentatonicMajor:
                return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 7) | (1u << 9);
            case ScaleType::PentatonicMinor:
                return (1u << 0) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 10);
        }
        return 0xFFF;
    }

    TonalSync() { setScale(0, ScaleType::Major); }

private:
    std::atomic<int32_t> m_root{0};
    std::atomic<uint32_t> m_pattern{0};
    std::atomic<uint32_t> m_scaleType{0};
};


} // namespace Aura::Core::Engine
