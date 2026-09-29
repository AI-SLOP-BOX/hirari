# 📊 SKILL: Hirari DSP Analysis (Professional Monitoring)

## 🤖 AI Role: Support Only (補助)
AIはあくまで分析結果の「提案者」に留まります。AIに自動マスタリングを丸投げしたり、最終的な書き出し判断をAIが単独で行うような実装は行いません。

This folder contains precision metering and audio analytics tools.

## 📁 Key File Responsibilities
- `loudness_meter.hpp`: EBU R128 LUFS and 4x True-peak mastering.
- `phase_correlator.hpp`: L/R phase relationship [-1, +1] monitor.
- `mastering_reference.hpp`: A/B comparison with LUFS matching.
- `spectrum_analyzer.hpp`: FFT-based real-time frequency distribution.
- `transient_detector.hpp`: High-pass filtered energy-flux onset detection.
- `phase_correlator.hpp`: L/R phase relationship and mono compatibility check.
- `rms_analyzer.hpp`: Fast and slow average RMS monitoring.

## 🛡️ Monitoring Rules (Measurement Accuracy)
- **EBU R128 / ITU-R BS.1770 Standards**: All LUFS analysis MUST implement K-Weighting filters (+4dB High Shelf, RLB High Pass).
- **TRUE PEAK ESTIMATION (4x OVERSAMPLING)**: To catch inter-sample peaks, use cascaded `Oversampler2x` upsamplers before peak detection.
- **WINDOWING (Vann/Hann/Blackman)**: For FFT analysis, always apply a smoothing window to window edges to prevent spectral leakage.

## 🏗️ Analytics Implementation
1. **Integrated LUFS**:
   - Long-term high-precision energy accumulation. Omit silent sections using the -70 LUFS gate.
2. **Transient Detection**:
   - Use high-pass energy flux to prioritize percussive onsets over mid-range sustain.
3. **Phase Correlation**:
   - Output between -1 (out of phase) and +1 (fully mono). 

## 🧪 AI Verification Directives
- Test `LoudnessMeter` accuracy with EBU test signals.
- Verify `TransientDetector` reliability across varying genre samples.
- Check `SpectrumAnalyzer` bins for accurate frequency resolution.
- Confirm 32-bit float precision in long-term energy accumulation.
