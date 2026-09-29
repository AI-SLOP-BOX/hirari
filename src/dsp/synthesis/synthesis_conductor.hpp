#pragma once

#include <memory>
#include <map>
#include "virtuoso_orchestra.hpp"
#include "synthesis_core.hpp"

namespace Hirari::Core::DSP::Synthesis {

/**
 * @brief SynthesisConductor: Master dispatcher for all synthesis engines.
 * Routes MIDI events to appropriate physical modeling, sampling, or synthesis cores.
 */
class SynthesisConductor {
public:
    explicit SynthesisConductor(double sampleRate) : m_sampleRate(sampleRate) {
        m_orchestra = std::make_unique<VirtuosoOrchestra>(sampleRate);
        m_subtractive = std::make_unique<SubtractiveSynth>(sampleRate);
    }

    /**
     * @brief Interprets a musical trigger and sends it to the best matching engine.
     */
    void triggerNote(const std::string& instrumentType, float pitch, float velocity) {
        if (instrumentType == "Piano" || instrumentType == "Brass") {
            auto model = (instrumentType == "Piano") ? VirtuosoOrchestra::Model::Piano 
                                                   : VirtuosoOrchestra::Model::Brass;
            m_orchestra->noteOn(model, pitch, velocity);
        } else {
            // Default to subtractive synthesis for lead/bass sounds
            m_subtractive->noteOn(pitch, velocity);
        }
    }

    /**
     * @brief Mixes all synthesis layers into the main output buffers.
     */
    void process(float* l, float* r, size_t numFrames) {
        m_orchestra->render(l, r, numFrames);
        m_subtractive->render(l, r, numFrames);
    }

private:
    double m_sampleRate;
    std::unique_ptr<VirtuosoOrchestra> m_orchestra;
    std::unique_ptr<SubtractiveSynth> m_subtractive;
};

} // namespace Hirari::Core::DSP::Synthesis
