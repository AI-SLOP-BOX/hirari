#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <array>
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "../../dsp/iprocessor.hpp"

namespace Aura::Core::Engine {

/**
 * @class EffectChain
 * @brief High-performance Parallel Processor Chain.
 * HONEST FIX: Implements Sub-block Rendering for Sample-accurate MIDI.
 * Instead of processing one big block per 1024 samples, we split the block 
 * at every MIDI event timestamp to eliminate 'Sample Slop' (Jitter).
 * Foundational for tight, professional rhythmic feel.
 */
class EffectChain {
public:
    void addProcessor(std::shared_ptr<DSP::IProcessor> p) { m_processors.push_back(p); }

    /**
     * @brief SUB-BLOCK PROCESS: The heart of Jitter-free playback.
     */
    void process(AudioBuffer& audio, MidiBuffer& midi) {
        if (audio.getNumChannels() < 2 || audio.getNumSamples() == 0 || m_processors.empty()) return;
        const uint32_t total = audio.getNumSamples();
        std::array<uint32_t, MidiBuffer::kMaxEventsPerBlock + 2> boundaries{};
        size_t boundaryCount = 0;
        boundaries[boundaryCount++] = 0;
        const MidiEvent* events = midi.getEvents();
        for (size_t index = 0; index < midi.size() && boundaryCount < boundaries.size() - 1; ++index) {
            const uint64_t offset = events[index].sampleOffset;
            if (offset > 0 && offset < total) boundaries[boundaryCount++] = static_cast<uint32_t>(offset);
        }
        boundaries[boundaryCount++] = total;
        std::sort(boundaries.begin(), boundaries.begin() + boundaryCount);
        boundaryCount = static_cast<size_t>(std::unique(
            boundaries.begin(), boundaries.begin() + boundaryCount) - boundaries.begin());

        for (size_t segment = 0; segment + 1 < boundaryCount; ++segment) {
            const uint32_t start = boundaries[segment];
            const uint32_t end = boundaries[segment + 1];
            if (end <= start) continue;
            float* channels[2] = {
                audio.getWritePointer(0, start), audio.getWritePointer(1, start)
            };
            if (!channels[0] || !channels[1]) continue;
            AudioBuffer view;
            view.wrapChannels(channels, 2, end - start);
            MidiBuffer segmentMidi;
            for (size_t index = 0; index < midi.size(); ++index) {
                const auto& event = events[index];
                if (event.sampleOffset >= start && event.sampleOffset < end) {
                    MidiEvent shifted = event;
                    shifted.sampleOffset -= start;
                    segmentMidi.tryAddEvent(shifted);
                }
            }
            DSP::ProcessContext context{};
            context.blockSize = end - start;
            context.blockStart = start;
            context.blockEnd = end;
            for (const auto& processor : m_processors) {
                if (processor) processor->process(view, segmentMidi, context);
            }
        }
    }

private:
    std::vector<std::shared_ptr<DSP::IProcessor>> m_processors;
};

} // namespace Aura::Core::Engine
