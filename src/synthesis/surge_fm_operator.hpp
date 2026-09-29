#pragma once
#include <cmath>

namespace Hirari::Synthesis {

/**
 * @class SurgeFmOperator
 * @brief 【究極の肉付け・新音源群】Surge XT譲りの『Phase Modulation（FM）オペレーター』。
 * ダブステップやベースミュージックにおいて不可欠な「金属的で鋭い、ガラスが割れるようなサウンド群」
 * （Growl/Wobble Bass等）を生み出すための、ピュアなFMシンセシス・アルゴリズムの中核モジュールです。
 */
class SurgeFmOperator {
public:
    SurgeFmOperator(double sr = 44100.0) : m_sampleRate(sr), m_phase(0.0) {}

    // 周波数とFMアルゴリズム用のRatio（倍率）をセット（例：比率 1:2 など）
    void setFrequency(double freq, float ratio = 1.0f) {
        m_baseFreq = freq;
        m_ratio = ratio;
        m_phaseIncrement = (m_baseFreq * m_ratio) / m_sampleRate;
    }

    /**
     * @brief モジュレータ（別オシレーター）からの変調を受け取りながら超高速に波形を生成する
     * @param fmModulation 別の発振器から流れ込んでくる変調用オーディオ信号（-1.0 ～ +1.0）
     * @param modDepth FMのかかり具合（FM Index）。これを素早いエンベロープ（ADSR）で動かすと鋭いアタックが鳴る
     */
    float process(float fmModulation = 0.0f, float modDepth = 0.0f) {
        // 【Surge XTのFMの中核ルール（フェーズモジュレーション）】
        // 自分自身の基本「進行フェーズ（m_phase）」に対して、外部からの「揺れ」を瞬時に足し合わせ、
        // その狂ったフェーズでサイン波（またはウェーブテーブル）をルックアップして音をひしゃげさせる。
        float modulatedPhase = m_phase + (fmModulation * modDepth);
        
        // 位相を 0.0〜1.0 の安全な範囲に強引にラップアラウンド（高速化のため簡易fmod展開）
        while (modulatedPhase >= 1.0f) modulatedPhase -= 1.0f;
        while (modulatedPhase < 0.0f)  modulatedPhase += 1.0f;

        // 計算量は限りなく少ないが、modDepthが大きいほど「非可逆的な倍音（ハーモニクス）」が無限に増殖し、
        // 凶悪なデジタルシンセサウンドが出力される仕組み。
        float out = std::sin(2.0f * M_PI * modulatedPhase);

        // キャリア（基本）フェーズの自律進行
        m_phase += m_phaseIncrement;
        if (m_phase >= 1.0) m_phase -= 1.0;

        return out;
    }

private:
    double m_sampleRate;
    double m_phase;
    double m_phaseIncrement = 0.0;
    double m_baseFreq = 440.0;
    float m_ratio = 1.0f; // オペレーターの周波数比率（例：キャリア=1、モジュレータ=2 など）
};

} // namespace Hirari::Synthesis
