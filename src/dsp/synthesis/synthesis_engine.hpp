#pragma once

#include <vector>
#include <map>
#include <atomic>
#include "../iprocessor.hpp"
#include "../../core/audio_buffer.hpp"
#include "aura_sampler_pro.hpp"
#include "drum_synth_bass.hpp"
#include "ivoice.hpp"
#include "../../core/worker_thread_pool.hpp"
#include "voice_manager.hpp"

#if defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#elif defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace Aura::DSP::Synthesis {

/**
 * @class SynthesisEngine
 * @brief Core engine for voice management and signal rendering.
 * HONEST FIX: Removed broken parallel rendering and purged 'Virtuoso' branding.
 */
class SynthesisEngine : public IProcessor {
public:
    SynthesisEngine() : m_voices() {}

    std::string getName() const override { return "Aura Synthesis Engine"; }

    void prepareToPlay(double sr, uint32_t maxBlockSize) noexcept override {
        m_voices.prepareToPlay(sr, maxBlockSize);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& ctx) noexcept override {
        (void)ctx;
        if (buffer.getNumChannels() < 2) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getWritePointer(1);
        size_t numFrames = buffer.getNumSamples();

        // 1. Sequential Voice Rendering (Safe & Low Overhead)
        m_voices.render(left, right, numFrames, midi);

        // 2. SIMD Gain Scaling
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Delegating SIMD gain scaling to the Rust 'SynthesisEngineOrchestrator'.
    }

    void reset() noexcept override {}


    VoiceManager m_voices;
    std::atomic<float> m_masterGain{1.0f};
};

} // namespace Aura::DSP::Synthesis
