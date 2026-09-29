# 🏎️ SKILL: Hirari Core Engine Architecture

## 🤖 AI Role: Support Only (補助)
**AIのエージェント化・自動化の禁止**: 本プロジェクトにおいてAIの役割は「補助の補助」であり、設計の最終決定権は常に人間にあります。AIが勝手に大規模な構造変更を行ったり、自律的にエージェントを構築することは許可されません。

This folder contains the "Nervous System" of the DAW. High stability, zero-latency jitter, and thread safety are paramount.

## 📁 Key File Responsibilities
- `timeline_system.hpp`: High-performance multi-core track summing, PDC alignment, and Render-in-Place.
- `track.hpp`: Track-level regions, effects, VCA gain, and Global Sidechain routing.
- `param_tree.hpp`: Lock-free atomic parameter table ($O(1)$).
- `automation_curve.hpp`: Bezier curves with Trim/Relative mode support.
- `take_management.hpp`: O(1) cached Comping system with automatic crossfades.
- `marker_system.hpp`: Global project structure (Intro, Verse, Chorus) and SMPTE navigation.
- `video_system.hpp`: Professional frame-accurate (24, 25, 29.97, 30 fps) timecode sync.
- `scale_system.hpp`: Global project key and harmonic quantization logic.
- `mix_snapshot.hpp`: Scene-recall and A/B mix comparison snapshots.
- `midi_transformer.hpp`: Logical MIDI editor for batch Humanize, Transpose, and Legato.
- `region_slicer.hpp`: Transient-based audio chopping (Flex Slice infrastructure).

## 🛡️ The "Zero Allocation" Protocol (Absolute Rule)
AI agents MUST NEVER introduce heap allocations (`new`, `std::vector::push_back`, `std::make_shared`) inside any function called by `TimelineSystem::render`.
- **Pre-allocation**: All buffers, task contexts, and effect chains must be resized BEFORE playback starts.
- **Lock-Free Sync**: Use `std::atomic` for cross-thread parameter updates. Never use `std::mutex` in the audio thread.

## 🏗️ Crucial Implementation Patterns
1. **PDC (Plugin Delay Compensation)**:
   - When adding effects to a `Track`, update its reported latency.
   - `TimelineSystem` aligns tracks by delaying faster ones using `DelayLine`.
2. **Bezier Fades**:
   - `AudioRegion` uses `FadeEnveloper` for musical S-curves at start/end.
3. **VCA Hierarchy**:
   - VCA gains are multiplicative. `Track::renderToLocal` scans up the `m_vcaMaster` chain to calculate total gain per frame.

## 🧪 AI Unit Test Directives
- Test `ParamTree` concurrent access under heavy load.
- Verify `TimelineSystem` job distribution across multiple CPU cores.
- Confirm sample-accurate alignment of `AudioRegion` playback at various offsets.
