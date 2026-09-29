#pragma once

#include "midi_buffer.hpp"
#include "rust_ffi.hpp"

namespace Hirari::Core {

// Stable C++ engine boundary; the preview synth state and DSP live in Rust.
class PreviewSynth {
public:
    PreviewSynth() : state_(hirari_preview_synth_create()) {}
    ~PreviewSynth() { hirari_preview_synth_destroy(state_); }
    PreviewSynth(const PreviewSynth&) = delete;
    PreviewSynth& operator=(const PreviewSynth&) = delete;

    void setEngine(uint32_t engine) { hirari_preview_synth_set_engine(state_, engine); }
    void reset() { hirari_preview_synth_reset(state_); }
    void process(const MidiEvent* events, size_t eventCount, float* left, float* right,
                 uint32_t frames, uint64_t playhead, double sampleRate, float morph,
                 double detuneRatio, float drive, float cutoff, float resonance,
                 float outputGain) {
        hirari_preview_synth_process(state_, events, eventCount, left, right, frames,
                                     playhead, sampleRate, morph, detuneRatio, drive,
                                     cutoff, resonance, outputGain);
    }

private:
    void* state_;
};

} // namespace Hirari::Core
