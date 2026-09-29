#include "hirari_sampler_pro.hpp"

namespace Hirari::Core::DSP::Synthesis {

void HirariSamplerPro::process(float* outputLeft, float* outputRight, size_t frames) {
    if (m_state) hirari_poly_sampler_process(m_state, outputLeft, outputRight, frames, true);
}

void HirariSamplerPro::processAdditive(float* outputLeft, float* outputRight, size_t frames) {
    if (m_state) hirari_poly_sampler_process(m_state, outputLeft, outputRight, frames, false);
}

} // namespace Hirari::Core::DSP::Synthesis
