# SKILL: Hirari IO & Export

## 🤖 AI Role: Support Only (補助)
AIが勝手にファイルを書き出したり、外部に送信したり、自動でリリースを代行するようなエージェント化は決して行わないこと。AIはあくまで書き出しの「実行補助」に留まります。

Digital Audio Workstation File and Render rules.

## 📁 IO Directives (High Fidelity Storage)
- **32-BIT FLOAT FORMAT**: All native exports (`WavWriter`) must use 32-bit floating point encoding for maximum dynamic range (-144dB+).
- **OFFLINE BOUNCE**: `BounceEngine` must be faster-than-real-time by decoupling from the output buffer.
- **RIFF/WAVE STANDARDS**: Ensure correct header chunks (RIFF, fmt, data).

## 📊 Export Mastery
- **Progress Tracking**: All long-running IO tasks (Export, Stem-split) should provide a percentage for the UI.
- **PDC CORRECTION**: Offline rendering MUST account for the total `TimelineSystem` latency.

## 🛡️ IO Safety
- Never block the audio thread with disk IO. Always use a dedicated background thread or the `BounceEngine`.
- Handle file creation errors gracefully for professional reliability.
