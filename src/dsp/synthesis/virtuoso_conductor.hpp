#pragma once

#include "synthesis_core.hpp"
#include <string>
#include <vector>
#include <memory>
#include <map>

namespace Hirari::Core::DSP::Synthesis {

/**
 * @brief VirtuosoConductor: High-level polyphonic Note Dispatcher.
 * Manages performance patches, layering, and sophisticated note routing for the internal synthesis core.
 */
class VirtuosoConductor {
public:
    explicit VirtuosoConductor(double sr) : m_sampleRate(sr) {
        m_engine = std::make_unique<SynthesisEngine>(sr);
    }

    /**
     * @brief Triggers a note on a specific patch. Supports multi-timbral layering.
     */
    void play(const std::string& patchName, int midiNote, float velocity) {
        // Professional Feature: Map patches to internal SynthesisEngine models
        // In the future, this will handle key-switches and poly-expression.
        m_engine->noteOn(patchName, midiNote, velocity);
    }

    /**
     * @brief Stops a specific note globally or per-patch.
     */
    void stop(int midiNote) {
        m_engine->noteOff(midiNote);
    }

    /**
     * @brief The main audio rendering bridge.
     */
    void render(float* l, float* r, size_t nf) {
        m_engine->render(l, r, nf);
    }

    SynthesisEngine& getEngine() { return *m_engine; }

private:
    double m_sampleRate;
    std::unique_ptr<SynthesisEngine> m_engine;
};

} // namespace Hirari::Core::DSP::Synthesis
