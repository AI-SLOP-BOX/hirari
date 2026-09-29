#pragma once
#include <vector>
#include <deque>
#include <cstdint>
#include <string>
#include <memory>
#include <iterator>
#include <cmath>
#include "../../core/audio_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/**
 * @class SpectralProcessor
 * @brief iZotope RX / SpectralLayers style 2D Frequency Editor.
 * HONEST FIX: Implements Spectral Lasso and Region-specific processing.
 * Users can 'draw' on the spectrogram to isolate or remove specific 
 * frequencies at specific times.
 */
class SpectralProcessor {
public:
    SpectralProcessor() = default;
    ~SpectralProcessor() { hirari_spectral_history_destroy(m_history); }
    SpectralProcessor(const SpectralProcessor&) = delete;
    SpectralProcessor& operator=(const SpectralProcessor&) = delete;
    SpectralProcessor(SpectralProcessor&&) = delete;
    SpectralProcessor& operator=(SpectralProcessor&&) = delete;

#include "spectral_processor_public_part_1.inc"
#include "spectral_processor_public_part_2.inc"
#include "spectral_processor_private.inc"

} // namespace Hirari::DSP::Analysis
