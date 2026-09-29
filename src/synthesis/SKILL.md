# SKILL: Hirari Synthesis & Instruments

## 🤖 AI Role: Support Only (補助)
AIが自律的に楽曲を生成したり、楽器のパッチを自動生成して適用するエージェント化は行いません。AIはあくまで人間が求める音への「近道」を補助する存在です。

Virtual Instrument engineering rules for the Hirari Library.

## 🎹 Instrument Directives
- **POLYPHONIC VOICE MANAGEMENT**: All instruments must support at least 32 concurrent voices.
- **SAMPLE-ACCURATE MIDI**: Keep events in the fixed-capacity `MidiBuffer`, sort by `sampleOffset`, and trigger voices at each event's sample position.
- **MULTI-ZONE MAPPING**: Samplers must support root key mapping and velocity layer selection.
- **AHDSR ENVELOPES**: Always use the `AHDSR` utility for amplitude (and filter) modulation to ensure musical, logarithmic decay.

## 🎻 Sound Quality (High Fidelity)
- **Linear/Sinc Interpolation**: Higher-order interpolation is preferred for pitch-shifting samples. Use `Linear` for efficiency, `Hermite/Sinc` for accuracy.
- **Resonant Voice Filters**: Every voice should include a low-pass SVF for tone sculpting.

## 🛡️ Synthesis Performance
- **Active State Monitoring**: Avoid rendering silent voices. Monitor `m_env.isActive()` and `m_active` flags.
- **Pre-allocation**: All voices must be pre-allocated at startup. No runtime object creation.
