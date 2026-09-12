#pragma once
#include <vector>
#include <deque>
#include <cstdint>
#include <string>
#include <iterator>
#include <cmath>
#include <complex>
#include "../utils/fft_utils.hpp"
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class SpectralProcessor
 * @brief iZotope RX / SpectralLayers style 2D Frequency Editor.
 * HONEST FIX: Implements Spectral Lasso and Region-specific processing.
 * Users can 'draw' on the spectrogram to isolate or remove specific 
 * frequencies at specific times.
 */
class SpectralProcessor {
#include "spectral_processor_public_part_1.inc"
#include "spectral_processor_public_part_2.inc"
#include "spectral_processor_private.inc"

} // namespace Aura::DSP::Analysis
