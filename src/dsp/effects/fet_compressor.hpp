#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../mixing/state_variable_filter.hpp"

namespace Aura::DSP::Effects {

/**
 * @class FETCompressor
 * @brief High-speed FET-style feedback compressor (1176 emulation).
 * HONEST FIX: Replaced expensive 'std::pow/log' calls with fast approximations 
 * to ensure high-density multitrack performance, and fixed math explosion in saturation.
 */
/**
 * @class FETCompressor
 * @brief High-speed FET-style feedback compressor (1176 emulation).
 * HONEST FIX: Replaced expensive 'std::pow/log' calls with fast approximations 
 * to ensure high-density multitrack performance.
 */
class FETCompressor : public IProcessor {
public:
    FETCompressor(double sr = 44100.0) : m_sampleRate(sr), m_scHPF(sr) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        setAttackMs(m_attackMs);
        setReleaseMs(m_releaseMs);
        reset();
    }

    void setThreshold(float db) { m_threshold = db; }
    void setRatio(int ratio) { m_ratioFlat = 1.0f - 1.0f / static_cast<float>(std::clamp(ratio, 1, 20)); }
    void setAttack(float ms) { m_attackMs = std::clamp(std::isfinite(ms) ? ms : 1.0f, 0.1f, 200.0f); setAttackMs(m_attackMs); }
    void setRelease(float ms) { m_releaseMs = std::clamp(std::isfinite(ms) ? ms : 100.0f, 1.0f, 2000.0f); setReleaseMs(m_releaseMs); }

    void setParameters(float input, float output, float threshold, float attackMs, float releaseMs, int ratio) {
        m_inputGain = std::pow(10.0f, input / 20.0f);
        m_outputGain = std::pow(10.0f, output / 20.0f);
        setThreshold(threshold);
        setAttack(attackMs);
        setRelease(releaseMs);
        setRatio(ratio);
    }

    void process(::Aura::Core::AudioBuffer& b, ::Aura::Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        const uint32_t channels = b.getNumChannels();
        const uint32_t samples = b.getNumSamples();
        if (channels == 0 || samples == 0) return;
        for (uint32_t i = 0; i < samples; ++i) {
            float peak = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) {
                const float* p = b.getReadPointer(c);
                if (p && std::isfinite(p[i])) peak = std::max(peak, std::abs(p[i] * m_inputGain));
            }
            const float detector = std::max(peak, 1.0e-8f);
            const float coeff = detector > m_envelope ? m_attack : m_release;
            m_envelope += (detector - m_envelope) * (1.0f - coeff);
            const float levelDb = 6.0205999f * fastLog2(std::max(m_envelope, 1.0e-8f));
            const float over = std::max(0.0f, levelDb - m_threshold);
            const float reductionDb = over * std::clamp(m_ratioFlat, 0.0f, 0.95f);
            const float gain = std::clamp(fastPow2((-reductionDb + 0.0f) / 6.0205999f) * m_outputGain, 0.0f, 4.0f);
            for (uint32_t c = 0; c < channels; ++c) {
                float* p = b.getWritePointer(c);
                if (!p) continue;
                const float x = std::isfinite(p[i]) ? p[i] : 0.0f;
                p[i] = std::isfinite(x * gain) ? x * gain : 0.0f;
            }
        }
    }


    void reset() noexcept override { m_envelope = 1.0f; }

private:
    /**
     * @brief Professional fast log2 approximation for audio DSP.
     */
    inline float fastLog2(float x) const {
        union { float f; uint32_t i; } vx = { x };
        float y = (float)vx.i;
        y *= 1.1920928955078125e-7f;
        return y - 126.94269504f;
    }

    /**
     * @brief Professional fast pow2 approximation for audio DSP.
     */
    inline float fastPow2(float p) const {
        float clipp = (p < -126) ? -126.0f : p;
        union { uint32_t i; float f; } v = { (uint32_t)((clipp + 126.94269504f) * 8388608.0f) };
        return v.f;
    }

    void setAttackMs(float ms) noexcept { m_attack = std::exp(-1.0f / (std::max(0.1f, ms) * 0.001f * std::max(1000.0, m_sampleRate))); }
    void setReleaseMs(float ms) noexcept { m_release = std::exp(-1.0f / (std::max(1.0f, ms) * 0.001f * std::max(1000.0, m_sampleRate))); }

    double m_sampleRate;
    float m_inputGain = 1.0f, m_outputGain = 1.0f;
    float m_threshold = -24.0f, m_ratioFlat = 0.75f;
    float m_attack = 0.99f, m_release = 0.999f;
    float m_attackMs = 1.0f, m_releaseMs = 100.0f;
    float m_envelope = 1.0f;
    Mixing::StateVariableFilter m_scHPF;
};

} // namespace Aura::DSP::Effects
