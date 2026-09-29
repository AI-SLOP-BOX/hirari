# 🎚️ SKILL: Hirari DSP Mixing (Pre-Mastering)

## 🤖 AI Role: Support Only (補助)
AIの自律的な意思決定を禁じます。音楽制作の「核心」に関わる調整や、ミキシングのエージェント化は行わないでください。AIはあくあでエンジニアを助けるツールとして振る舞うこと。

This folder contains dynamic and spectral processors for individual tracks and buses.

## 📁 Key File Responsibilities
- `state_variable_filter.hpp`: Industrial Zero-Delay Feedback (ZDF) SVF.
- `pro_limiter.hpp`: 2ms Look-ahead True-Peak mastering limiter.
- `fet_compressor.hpp`: Compatibility alias for the Rust-backed FET processor in `effects/fet_compressor.hpp`.
- `pitch_corrector.hpp`: Autotune-style vocal pitch snapping with Scale integration.
- `vocal_doubler.hpp`: Artificial double-tracking and stereo widening.
- `noise_gate.hpp`: Hysteresis and hold logic dynamics gate.
- `de_esser.hpp`: Frequency-selective sidechain vocal de-esser.
- `master_suite.hpp`: Multi-band compressor and final processing chain.

## 🛡️ Mixing Rules (Audio Quality)
- **ZDF FILTER STABILITY**: Always use the SVF method for predictable resonant behavior across frequency sweeps.
- **LOOK-AHEAD DYNAMICS**: Mastering-grade effects MUST use `DelayLine` to detect transients BEFORE the gain envelope changes, ensuring no overshoot.
- **STEREO INDEPENDENCE**: All processors must handle stereo pairs (`L/R`) independently or with linked detectors as needed.

## 🏗️ Implementation Guidelines
1. **Sidechaining**:
   - Implement `processWithSidechain` for gated effects and sidechain ducking.
2. **Gain Smoothing**:
   - Never snap gain values. Use internal exponential smoothing for all dynamic attenuations.
3. **PDC Alignment**:
   - Correctly report `getLatency()` for all look-ahead processors to avoid phasing issues.

## 🧪 AI Verification Directives
- Test `TruePeakLimiter` against inter-sample clip samples.
- Verify `StateVariableFilter` stability at Nyquist.
- Check `DeEsser` sidechain filter response with pink noise.
