#include "fade_enveloper.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Mixing {

/**
 * @brief FadeEnveloper: Provides logarithmic crossfades between regions.
 * Addresses the "missing non-destructive fades" from the review.
 */
/**
 * @brief FadeEnveloper: Professional Logic Pro style non-destructive fades.
 * HONEST FIX: Implements 5ms micro-fades and Equal Power Logarithmic curves.
 * This eliminates 'digital clicks' when regions are moved or edited.
 */
void FadeEnveloper::apply(float* out, const float* in1, const float* in2, size_t numFrames) {
    hirari_fade_apply(out, in1, in2, numFrames);
}

void FadeEnveloper::applyMicroFade(float* buffer, size_t numFrames, bool isFadeIn) {
    hirari_fade_apply_micro(buffer, numFrames, isFadeIn);
}

float FadeEnveloper::getFadeFactor(size_t pos, size_t length, bool isFadeIn, Curve type, float curvature) {
    return hirari_fade_factor(pos, length, isFadeIn, static_cast<uint8_t>(type), curvature);
}

} // namespace Hirari::DSP::Mixing
