#include <cassert>
#include <cmath>
#include <limits>
#include <memory>

#include "../src/core/engine/modulator_system.hpp"
#include "../src/dsp/library/dsp_standard_pro.hpp"

int main() {
    using Aura::DSP::Library::DSPStandardPro;

    // Invalid and even-length FIR requests must fail instead of producing a
    // malformed centre tap or dividing by zero in the Blackman window.
    assert(DSPStandardPro::designLinearPhaseLP(1000.0, 48000.0, 64).coeffs.empty());
    const auto fir = DSPStandardPro::designLinearPhaseLP(1000.0, 48000.0, 65);
    assert(fir.coeffs.size() == 65);
    for (double coefficient : fir.coeffs) assert(std::isfinite(coefficient));
    DSPStandardPro::FIRProcessor processor;
    assert(processor.prepare(fir));
    float impulse[8] = {1.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};
    processor.processBlock(impulse, 8);
    for (float sample : impulse) assert(std::isfinite(sample));

    assert(DSPStandardPro::polyBLEPPro(0.2f, 0.0f) == 0.0f);
    assert(DSPStandardPro::softClipPro(std::numeric_limits<float>::quiet_NaN(), 1.0f) == 0.0f);
    assert(std::isfinite(DSPStandardPro::softClipPro(0.5f, 2.0f)));

    auto& modulators = Aura::Core::Engine::ModulatorSystem::getInstance();
    modulators.addModulator(17, std::make_shared<Aura::Core::Engine::LFO>(2.0f), 0.5f);
    const float value = modulators.getModulatedValue(17, 1.0f, 48000.0);
    assert(std::isfinite(value));
    assert(std::abs(modulators.getModulatedValue(999, 0.25f, 48000.0) - 0.25f) < 1.0e-6f);
    return 0;
}
