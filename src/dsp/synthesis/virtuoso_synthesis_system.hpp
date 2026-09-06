#pragma once

#include <memory>
#include <vector>
#include <algorithm>
#include <cmath>
#include "virtuoso_orchestra.hpp"
#include "aura_sampler_pro.hpp"
#include "synthesis_core.hpp"

namespace Aura::Core::DSP::Synthesis {

/**
 * @brief VirtuosoSynthesisSystem: The master synthesis hub of the DAW.
 * Orchestrates physical modeling, sampling, and subtractive synthesis into a single high-performance engine.
 */
class VirtuosoSynthesisSystem {
public:
    explicit VirtuosoSynthesisSystem(double sr) : m_sampleRate(sr) {
        m_orchestra = std::make_unique<VirtuosoOrchestra>(sr);
        m_sampler = std::make_unique<AuraSamplerPro>(sr);
        m_synth = std::make_unique<SubtractiveSynth>(sr);
    }

    /**
     * @brief High-level trigger for complex instrument patches.
     */
    void triggerInstrument(const std::string& patchName, float pitch, float velocity) {
        // PROFESSIONAL RULE: A single "Patch" can trigger multiple engines (Layering).
        if (patchName == "Hybrid Piano") {
            m_orchestra->noteOn(VirtuosoOrchestra::Model::Piano, pitch, velocity);
            m_sampler->noteOn(static_cast<uint32_t>(std::clamp(std::isfinite(pitch) ? pitch : 60.0f, 0.0f, 127.0f)),
                              std::clamp(std::isfinite(velocity) ? velocity * 0.5f : 0.0f, 0.0f, 1.0f));
        } else {
            m_synth->noteOn(pitch, velocity);
        }
    }

    /**
     * @brief Mixes and renders all synthesis layers (Real-time thread).
     */
    void render(float* l, float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0) return;
        m_orchestra->render(l, r, numFrames);
        m_sampler->processAdditive(l, r, numFrames);
        m_synth->render(l, r, numFrames);
    }

private:
    double m_sampleRate;
    std::unique_ptr<VirtuosoOrchestra> m_orchestra;
    std::unique_ptr<AuraSamplerPro> m_sampler;
    std::unique_ptr<SubtractiveSynth> m_synth;
};

} // namespace Aura::Core::DSP::Synthesis
