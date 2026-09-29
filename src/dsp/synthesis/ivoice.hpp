#pragma once

#include <vector>
#include <cstdint>

namespace Hirari::Core::DSP::Synthesis {

/**
 * @brief IVoice: The universal polymorphic base for all sound generators.
 * Addresses the "fragmented synth/sampler architecture" identifier from the review.
 */
class IVoice {
public:
    virtual ~IVoice() = default;
    
    virtual void trigger(float pitch, float velocity) = 0;
    virtual void release() = 0;
    virtual void process(float* output, size_t numFrames) = 0;
    virtual bool isActive() const = 0;
    
    // Virtual MIDI implementation
    virtual void noteOn(uint8_t note, uint8_t velocity) = 0;
    virtual void noteOff(uint8_t note) = 0;
    virtual uint8_t getNote() const = 0;
};

} // namespace Hirari::Core::DSP::Synthesis
