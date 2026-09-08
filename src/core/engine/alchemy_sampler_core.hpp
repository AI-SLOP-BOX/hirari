#include <stdint.h>
#include <random>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <array>
#include <complex>
#include "../../dsp/iprocessor.hpp"
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"

namespace Aura::Core::Engine {

/**
 * @struct SampleZone
 * @brief Mapping zone for a single sample (Key/Velocity).
 */
struct SampleZone {
    uint8_t lowKey, highKey;
    uint8_t lowVel, highVel;
    const float* data;
    uint64_t sampleCount;
    uint8_t rootKey = 60;
    double sampleRate = 44100.0;
};

/**
 * @class AlchemySamplerCore
 * @brief Industrial multi-engine sampling and synthesis core.
 * Orchestrates granular, additive, and spectral synthesis with multisample support.
 */
class AlchemySamplerCore : public ::Aura::DSP::IProcessor {
public:
    enum class EngineType { Granular, Additive, Spectral, Classic };

    AlchemySamplerCore() : m_engine(EngineType::Classic) {
        m_rng.seed(1337); // Standard seed for deterministic RT behavior if needed
    }

    // --- SYNTHESIS ENGINE ---
    void setEngine(EngineType type) { m_engine = type; }
    void process(::Aura::Core::AudioBuffer& buffer, ::Aura::Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& ctx) noexcept override;
    void prepareToPlay(double sr, uint32_t sz) noexcept override;
    void reset() noexcept override {
        m_grainSampleAccumulator = 0;
        for (auto& voice : m_voices) {
            voice.active.store(false, std::memory_order_release);
            voice.zone = nullptr;
            voice.pos = 0.0;
        }
        for (auto& grain : m_grainPool) {
            grain.active.store(false, std::memory_order_release);
            grain.data = nullptr;
            grain.pos = 0.0;
            grain.currentSample = 0;
        }
    }

    // --- MODULATION MATRIX ---
    struct ModulationSource {
        uint32_t type; // LFO, Envelope, Aftertouch
        float value;
    };
    void updateModulation(const std::vector<ModulationSource>& sources);

    // --- MAPPING ---
    void addZone(const SampleZone& zone);

    struct Voice {
        const SampleZone* zone = nullptr;
        double pos = 0.0;
        uint8_t note = 0;
        uint8_t channel = 0;
        float velocity = 0.0f;
        std::atomic<bool> active{false};
    };

    void processClassic(::Aura::Core::AudioBuffer& buffer, ::Aura::Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& ctx);
    void processGranular(::Aura::Core::AudioBuffer& buffer);
    void processAdditive(::Aura::Core::AudioBuffer& buffer);
    void processSpectral(::Aura::Core::AudioBuffer& buffer);

    struct Grain {
        const float* data = nullptr;
        uint64_t sampleCount = 0;
        double pos = 0.0;
        double step = 1.0;
        float duration = 0.0f;
        float pan = 0.5f;
        float velocity = 1.0f;
        uint32_t currentSample = 0;
        std::atomic<bool> active{false};
    };


    struct Oscillator {
        float phase = 0.0f;
        float freq = 0.0f;
        float amp = 0.0f;
        float y1 = 0.0f, y2 = 0.0f; // Recursive state
        float coeff = 0.0f;
    };


    EngineType m_engine;
    SampleZone m_zones[256];
    uint32_t m_zoneCount = 0;
    Voice m_voices[128];
    Grain m_grainPool[256];
    Oscillator m_oscBank[1024]; // High-density additive bank
    std::array<float, 128> m_modMatrix;
    std::mt19937 m_rng;
    uint64_t m_grainSampleAccumulator = 0;
    double m_sampleRate = 44100.0;
    
    // Pre-allocated workspace to guarantee zero dynamic allocations in spectral mode
    std::complex<float> m_spectralWorkspace[512];
};

} // namespace Aura::Core::Engine
