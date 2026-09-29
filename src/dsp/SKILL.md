# SKILL: Hirari DSP (Digital Signal Processing)

Advanced audio engineering rules for the Hirari DAW.

## 🎛️ DSP Directives (Audio Mastery)
- **K-WEIGHTING**: All loudness analysis must follow ITU-R BS.1770 K-Weighting filters (+4dB High Shelf, RLB High Pass).
- **ZDF (Zero-Delay Feedback)**: Preference is given to ZDF structure for filters (`StateVariableFilter`) to ensure analog-like stability and response.
- **OVERSAMPLING (Anti-Aliasing)**: Any non-linear process (Saturators, Limiters) MUST use at least 2x Oversampling (FIR Polyphase).
- **TRUE PEAK**: Mastering meters must use 4x oversampling interpolation to catch inter-sample peaks.

## 🥩 "Meat" Engineering (Professional Effects)
- **Fades & Envelopes**: Use Bezier/S-Curves for musical transitions. Fades must be sample-accurate.
- **Sidechaining**: Implement the `IProcessor` interface and use `processWithSidechain` for ducking/dynamic EQ.
- **Look-ahead Logic**: Use `DelayLine` for mastering processors (Limiters, De-essers) to catch transients before the gain envelope moves.

## 🛡️ DSP Safety
- Kill Denormals in all feedback loops.
- Use `std::numbers::pi_v<float>` for trigonometric constants.
- Avoid `std::abs` in smoothness checks; use sample-count decrementing for target values.
