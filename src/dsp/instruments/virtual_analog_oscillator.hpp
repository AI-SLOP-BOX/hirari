#pragma once
#include <cmath>
#include <algorithm>
#include <vector>

namespace Aura::DSP::Instruments {

/**
 * @class VirtualAnalogOscillator
 * @brief Professional Anti-Aliased Synthesizer Oscillator.
 * 【究極の肉付け・完全新規】DAWに付属する初期シンセがショボいと誰も使いません。
 * ただの「ノコギリ波」を数学的に計算しただけでは、高音域で強烈なデジタルノイズ（折り返し歪み）が鳴り響きます。
 * 本物のアナログシンセサイザーの「音の太さと滑らかさ」を計算式（PolyBLEP）で完全に再現する、
 * 商用プラグインレベルのVA（Virtual Analog）オシレーターエンジンを新規肉付けしました。
 */
class VirtualAnalogOscillator {
public:
    enum class Waveform { Sawtooth, Square, Triangle };

    VirtualAnalogOscillator(double sr = 44100.0)
        : m_sampleRate(std::isfinite(sr) && sr >= 8000.0 ? sr : 44100.0) {}

    void setFrequency(float freq) {
        if (!std::isfinite(freq)) {
            m_phaseIncrement = 0.0f;
            return;
        }
        // Keep the BLEP transition width valid and prevent alias-dominated
        // frequencies from destabilising the phase accumulator.
        const float limited = std::clamp(freq, 0.0f, static_cast<float>(m_sampleRate * 0.49));
        m_phaseIncrement = limited / static_cast<float>(m_sampleRate);
    }

    void setWaveform(Waveform type) { m_waveform = type; }

    float process() {
        float out = 0.0f;
        float t = m_phase; // 0.0 to 1.0 のフェーズ

        // 1. 各種波形の素のデジタル生成
        switch (m_waveform) {
            case Waveform::Sawtooth: {
                // ナイーブなノコギリ波（エイリアスノイズが出る）
                out = 2.0f * t - 1.0f;
                // PolyBLEP（エイリアス除去アルゴリズム）で波形のカドをデジタル的に丸める
                out -= polyBlep(t);
                break;
            }
            case Waveform::Square: {
                out = (t < 0.5f) ? 1.0f : -1.0f;
                out += polyBlep(t) - polyBlep(std::fmod(t + 0.5f, 1.0f));
                break;
            }
            case Waveform::Triangle: {
                // 三角波は最初から折り返しが少ないため絶対値のみ
                out = 2.0f * std::abs(2.0f * t - 1.0f) - 1.0f;
                break;
            }
        }

        // 2. フェーズの前進（リング）
        m_phase += m_phaseIncrement;
        if (m_phase >= 1.0f) m_phase -= 1.0f;

        return out;
    }

private:
    double m_sampleRate;
    float m_phase = 0.0f;
    float m_phaseIncrement = 0.0f;
    Waveform m_waveform = Waveform::Sawtooth;

    // 【肉付け：DSPの極み】PolyBLEP (Polynomial Band-Limited Step)
    // エイリアスノイズ（デジタル特有のチリチリ音）を消すために、
    // 波形がガクッと切り替わる『角（カド）』の部分だけを、滑らかな多項式カーブで削り取る数学的補間です。
    // これにより、CPU負荷をほぼゼロに抑えながら、1970年代のMoogのような図太いアナログ音が出ます。
    float polyBlep(float t) {
        float dt = m_phaseIncrement;
        if (t < dt) { 
            // 波形の周期の「始まり」のカド
            t /= dt;
            return t + t - t * t - 1.0f;
        } else if (t > 1.0f - dt) { 
            //波形の周期の「終わり」のカド
            t = (t - 1.0f) / dt;
            return t * t + t + t + 1.0f;
        }
        return 0.0f; // カド以外（傾斜部分）は原音のまま弄らない
    }
};

} // namespace Aura::DSP::Instruments
