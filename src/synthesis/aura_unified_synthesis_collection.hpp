#pragma once

#include <vector>
#include <string>
#include <cmath>
#include <atomic>
#include <map>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::Library::Synthesis {

/**
 * @brief UNIFIED SYNTHESIS COLLECTION: The Absolute Truth of Sound.
 * Renders multiple synthesizer models (FM, Electric Piano, Organ, Morphing Wavetable).
 */
class UnifiedSynthesisCollection {
public:
    enum class Model { 
        GrandPiano, VocalFormant, KotoPhysical, RetroWavetable, 
        FM808, VintageEP, B3Organ, OrchestraHarp, SineHit 
    };

    UnifiedSynthesisCollection() : m_phase(0.0f), m_noteAge(0.0f) {}

    /**
     * @brief Renders the designated synthesizer model.
     */
    void render(float* l, float* r, size_t numFrames, Model m, float frequency = 440.0f, double sr = 44100.0) {
        float phaseInc = (2.0f * M_PI * frequency) / static_cast<float>(sr);
        
        for (size_t i = 0; i < numFrames; ++i) {
            float out = 0;
            switch (m) {
                case Model::GrandPiano:   out = renderPiano(); break;
                case Model::VocalFormant: out = renderVocal(); break;
                case Model::KotoPhysical: out = renderKoto(); break;
                case Model::OrchestraHarp: out = renderHarp(); break;
                case Model::FM808:        out = renderFM808(); break;
                case Model::RetroWavetable: out = renderRetroWavetable(); break;
                case Model::VintageEP:    out = renderVintageEP(); break;
                case Model::B3Organ:       out = renderB3Organ(); break;
                case Model::SineHit:      out = renderSineHit(); break;
                default: out = 0.1f * std::sin(m_phase); break;
            }
            l[i] += out; r[i] += out;
            m_phase += phaseInc;
            if (m_phase > 2.0f * M_PI) m_phase -= 2.0f * M_PI;
            
            m_noteAge += 1.0f / static_cast<float>(sr);
        }
    }

    void triggerNote() { m_noteAge = 0; m_phase = 0; }

private:
    float renderPiano() { 
        return std::sin(m_phase) * std::exp(-m_noteAge * 2.0f) * 0.7f; 
    }

    float renderVocal() {
        float saw = (std::fmod(m_phase, 2.0f * M_PI) / M_PI) - 1.0f;
        return saw * 0.3f * std::exp(-m_noteAge * 0.5f);
    }

    float renderKoto() { 
        return std::sin(m_phase * 1.5f) * std::exp(-m_noteAge * 4.0f) * 0.5f; 
    }
    
    float renderHarp() { 
        return (std::sin(m_phase) + 0.5f * std::sin(m_phase * 2.0f)) * std::exp(-m_noteAge * 1.5f) * 0.2f; 
    }

    float renderFM808() {
        float modPhase = m_phase * 2.0f; 
        float modEnv = std::exp(-m_noteAge * 15.0f); 
        float modulator = std::sin(modPhase) * modEnv * 5.0f;
        float carrierPhase = m_phase + modulator;
        float carrierEnv = std::exp(-m_noteAge * 8.0f); 
        return std::sin(carrierPhase) * carrierEnv * 0.6f;
    }

    float renderRetroWavetable() {
        float phaseWrap = std::fmod(m_phase, 2.0f * M_PI);
        float t = phaseWrap / (2.0f * M_PI); 
        float sq = (t < 0.5f) ? 1.0f : -1.0f;
        float saw = 2.0f * t - 1.0f;
        float morph = std::min(m_noteAge * 2.0f, 1.0f);
        return (sq + (saw - sq) * morph) * 0.25f * std::exp(-m_noteAge * 1.5f);
    }

    float renderVintageEP() {
        float fundamental = std::sin(m_phase);
        float chime = std::sin(m_phase * 8.0f) * std::exp(-m_noteAge * 12.0f); 
        return (fundamental + chime * 0.3f) * std::exp(-m_noteAge * 2.0f) * 0.5f;
    }

    float renderB3Organ() {
        float f1 = std::sin(m_phase);      
        float f2 = std::sin(m_phase * 2.0f) * 0.5f; 
        float f3 = std::sin(m_phase * 3.0f) * 0.3f; 
        float f4 = std::sin(m_phase * 4.0f) * 0.2f; 
        return (f1 + f2 + f3 + f4) * 0.4f * std::exp(-m_noteAge * 0.8f);
    }

    float renderSineHit() {
        return std::sin(m_phase) * std::exp(-m_noteAge * 20.0f) * 0.8f;
    }

    float m_phase;
    float m_noteAge; 
};

/**
 * @brief UnifiedAssetLibrary: Manifest for Factory Patches.
 */
struct FactoryPatch { std::string name, category, path; };
class UnifiedAssetLibrary {
public:
    static UnifiedAssetLibrary& getInstance() { static UnifiedAssetLibrary i; return i; }
    std::vector<FactoryPatch> m_manifest = {
        {"Master Steinway", "Piano", "samples/piano_01.wav"},
        {"B3 Drawbar 888", "Organ", "samples/b3_888.wav"},
        {"Vocoder Lead", "Vocal", "samples/voc_01.wav"}
    };
};

} // namespace Aura::Library::Synthesis
