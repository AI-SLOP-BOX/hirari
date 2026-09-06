/*
 * Aura DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#pragma once
#include <cstdint>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>
#include "../core/audio_buffer.hpp"
#include "../core/midi_buffer.hpp"

namespace Aura::DSP {

/**
 * @struct ProcessContext
 * @brief THE ARCHITECTURAL COMPASS: Navigates time and sample-accurate events.
 * HONEST FIX: Added blockStart and blockEnd to handle loop-wraps and 
 * region boundaries mid-block (Logic Pro 11 Grade precision).
 */
struct ProcessContext {
    uint64_t playhead;
    uint64_t blockStart; // Absolute sample start of this block
    uint64_t blockEnd;   // Absolute sample end of this block
    double sampleRate;
    double bpm;
    uint32_t blockSize;
    uint64_t audioConfigGeneration = 0;
    bool isPlaying;
    bool anyoneSoloed;
    bool isLooping = false;                 // --- RT-SAFE LOOPING ---
    uint64_t cycleStart = 0;
    uint64_t cycleEnd = 0;
    bool isSeeking = false; 
    const Core::AudioBuffer* sidechainBuffer = nullptr;
    uint32_t numOutputChannels = 2;
    bool isSpatial = false;

    
    // Performance context for CPU monitoring
    mutable double cpuLoad = 0.0; 
};

/**
 * @interface IProcessor
 * @brief THE REAL-TIME CONTRACT: No exceptions, no allocations.
 */
class IProcessor {
public:
    struct ParameterDescriptor {
        float minimum = 0.0f;
        float maximum = 1.0f;
        bool stepped = false;
        bool valid() const noexcept {
            return std::isfinite(minimum) && std::isfinite(maximum) && minimum <= maximum;
        }
    };

    virtual ~IProcessor() = default;

    virtual void prepareToPlay(double sr, uint32_t bs) noexcept = 0;
    virtual void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept = 0;
    virtual void reset() noexcept = 0;
    
    virtual std::string getName() const { return "Processor"; }
    virtual uint32_t getLatencySamples() const noexcept { return 0; }
    virtual uint32_t getTailSamples() const noexcept { return 0; }
    
    // State Persistence (SBF-v5 Binary Snapshots)
    virtual std::vector<uint8_t> getState() const {
        // Every processor gets a small, versioned host-state envelope even if
        // it has no plugin-specific parameters. This preserves mix/bypass and
        // routing state across project reloads instead of silently resetting
        // legacy effects to defaults.
        constexpr uint32_t kMagic = 0x41555241u; // "AURA"
        constexpr uint16_t kVersion = 1;
        std::vector<uint8_t> state(16, 0);
        std::memcpy(state.data(), &kMagic, sizeof(kMagic));
        std::memcpy(state.data() + 4, &kVersion, sizeof(kVersion));
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data() + 6, &flags, sizeof(flags));
        std::memcpy(state.data() + 8, &m_mix, sizeof(m_mix));
        std::memcpy(state.data() + 12, &m_sidechainBusId, sizeof(m_sidechainBusId));
        return state;
    }
    /// Restore a persisted state blob on the control thread.  Returning the
    /// result is intentional: a checksum-valid blob can still be rejected by
    /// a plugin or fail during decoding, and callers must not confuse that
    /// with a successful restore.  Existing callers may ignore the result.
    virtual bool setState(const std::vector<uint8_t>& data) {
        if (data.size() != 16) return false;
        uint32_t magic = 0; uint16_t version = 0; uint16_t flags = 0;
        float mix = 0.0f; uint32_t sidechain = 0;
        std::memcpy(&magic, data.data(), sizeof(magic));
        std::memcpy(&version, data.data() + 4, sizeof(version));
        std::memcpy(&flags, data.data() + 6, sizeof(flags));
        std::memcpy(&mix, data.data() + 8, sizeof(mix));
        std::memcpy(&sidechain, data.data() + 12, sizeof(sidechain));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 ||
            !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        return true;
    }
    // GUI state is control-plane data owned by the plugin.  Keep empty
    // defaults so existing processors remain source-compatible while the
    // host can persist editor/window state when a plugin provides it.
    virtual std::vector<uint8_t> saveGuiState() const { return {}; }
    virtual bool loadGuiState(const std::vector<uint8_t>& /*data*/) { return true; }
    virtual bool restoreStateChecked(const std::vector<uint8_t>& data) {
        return setState(data);
    }

    // Parameter Interface
    virtual void setParameter(uint32_t /*id*/, float /*value*/) noexcept {}
    virtual float getParameter(uint32_t /*id*/) const noexcept { return 0.0f; }
    virtual uint32_t getNumParameters() const noexcept { return 0; }
    // Native-unit parameter metadata is optional for legacy processors. A
    // false return means the processor owns validation of the supplied value.
    virtual bool getParameterDescriptor(uint32_t /*id*/, ParameterDescriptor& out) const noexcept {
        out = {};
        return false;
    }
    virtual void getParameterName(uint32_t /*id*/, char* outName, uint32_t maxSize) const noexcept {
        if (outName && maxSize > 0) outName[0] = '\0';
    }
    // Unsupported parameters are not automated. Concrete processors must
    // opt in explicitly once they expose a real automation lane.
    virtual bool isParameterAutomated(uint32_t /*id*/) const noexcept { return false; }

    // Native editor availability is separate from audio processing readiness.
    virtual bool hasNativeEditor() const noexcept { return false; }
    // UI-thread native editor lifecycle. The parent handle is platform-owned
    // (HWND/NSView/X11 surface); audio processing must never call these.
    virtual uint64_t openNativeEditor(uintptr_t /*parent*/) noexcept { return 0; }
    virtual bool closeNativeEditor(uint64_t /*session*/) noexcept { return false; }

    // Control-thread diagnostic hook. Real-time processors publish edge-triggered
    // fault events atomically; consumers drain them outside the audio callback.
    virtual bool takeWatchdogTrip() noexcept { return false; }

    // Number of non-finite output samples sanitized at the plugin boundary.
    // This is a control/diagnostic read and must never allocate or block.
    virtual uint64_t nonFiniteSampleCount() const noexcept { return 0; }

    // Telemetry: RAM vs Disk streaming health (true = Streaming, false = RAM)
    virtual std::vector<bool> getStreamingHealth() const { return {}; }

    // Mix and Bypass
    void setMix(float mix) noexcept { m_mix = std::clamp(mix, 0.0f, 1.0f); }
    float getMix() const noexcept { return m_mix; }
    void setBypassed(bool b) noexcept { m_bypassed = b; }
    bool isBypassed() const noexcept { return m_bypassed; }

    // Sidechain
    void setSidechainBus(uint32_t busId) noexcept { m_sidechainBusId = busId; }
    uint32_t getSidechainBus() const noexcept { return m_sidechainBusId; }

protected:
    bool m_bypassed = false;
    float m_mix = 1.0f;
    uint32_t m_sidechainBusId = 0;
};

/**
 * @class PurityPassProcessor
 * @brief THE NULL OBJECT: Branches are for trees, let the audio flow.
 * Guaranteed NO-OP.
 */
class PurityPassProcessor : public IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer&, Core::MidiBuffer&, const ProcessContext&) noexcept override {}
    void reset() noexcept override {}
    std::string getName() const override { return "PurityPass"; }
};

} // namespace Aura::DSP
