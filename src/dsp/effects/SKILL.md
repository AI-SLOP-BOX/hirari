# 🎭 SKILL: Hirari DSP Effects (Boutique & Vintage)

## 🤖 AI Role: Support Only (補助)
AIによる音色操作の自律化は行いません。クリエイティブな実験やサウンドメイキングをAIが支配することを禁止します。

This folder contains high-end sonic sculpting and time-based effects.

## 📁 Key File Responsibilities
- `lush_reverb.hpp`: High-end Hall/Chamber Algorithmic Reverb (FDN).
- `analog_saturator.hpp`: C++ host adapter for Rust-owned saturation models and 2x oversampling.
- `vintage_eq.hpp`: Pultec-style passive EQ (Low-end trick).
- `multiband_exciter.hpp`: LR-4 crossover based frequency saturation.
- `elastic_audio.hpp`: Multi-mode Time-Stretch (Poly/Mono/Percussive).
- `delay_line.hpp`: High-performance circular buffer.
- `hirari-core-bridge/src/oversampler.rs`: Stateful two-branch all-pass 2x interpolation used by the Rust saturator.

## 🛡️ Effects Selection & Processing Rules
- **NON-LINEAR OVERSAMPLING (OVERSAMPLER2X)**: Non-linear effects (Saturators, Distortions) MUST use 2x oversampling to prevent digital aliasing.
- **2X ALL-PASS INTERPOLATION**: The saturator uses fixed complementary all-pass branches and sample-accurate Rust-owned state.
- **HADAMARD DECORRELATION**: For Reverb, use the Hadamard matrix for dense, musical reverb tails without resonant metallic clusters.

## 🏗️ Analog-Modeling Implementation
1. **Tube/Tape Saturation**:
   - Model even and odd harmonics. Use bias offsets for warmth.
2. **Vintage EQ (Pultec)**:
   - Simultaneous boost and cut at the same frequency for the "Low End Trick".
3. **Stereo Image Preservation**:
   - Always process discretely or use M/S (Mid-Side) matrix for Width control.

## 🧪 AI Verification Directives
- Test `AnalogSaturator` harmonic profile for aliasing.
- Verify `LushReverb` feedback stability with unit impulses.
- Check `VintageEQ` magnitude response at 60Hz crossover.
- Ensure `DenormalKiller` is used in all reverb/delay feedback lines.
