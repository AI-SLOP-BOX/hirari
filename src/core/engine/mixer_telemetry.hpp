#pragma once

#include <cstdint>
#include <cstddef>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

class MixerTelemetryHub {
public:
    static constexpr uint32_t kMaxTracks = 4096;

    struct Snapshot {
        float peakLeft = 0.0f;
        float peakRight = 0.0f;
        float rmsLeft = 0.0f;
        float rmsRight = 0.0f;
        uint32_t clippingCount = 0;
        float dcOffsetLeft = 0.0f;
        float dcOffsetRight = 0.0f;
        float phaseCorrelation = 0.0f;
        float loudnessLufs = -120.0f;
        float spectrumRms[8]{};
    };
    static_assert(sizeof(Snapshot) == sizeof(HirariMixerTelemetrySnapshot));

    MixerTelemetryHub() : m_state(hirari_mixer_telemetry_create()) {}
    ~MixerTelemetryHub() { hirari_mixer_telemetry_destroy(m_state); }

    MixerTelemetryHub(const MixerTelemetryHub&) = delete;
    MixerTelemetryHub& operator=(const MixerTelemetryHub&) = delete;

    const void* nativeState() const noexcept { return m_state; }

    void pushAudioBlock(uint32_t trackId, const float* left, const float* right,
                        uint32_t frames) noexcept {
        hirari_mixer_telemetry_push(m_state, trackId, left, right, frames);
    }

    bool readSnapshot(uint32_t trackId, Snapshot& output) const noexcept {
        HirariMixerTelemetrySnapshot snapshot{};
        if (!hirari_mixer_telemetry_read(m_state, trackId, &snapshot)) return false;
        output.peakLeft = snapshot.peak_l;
        output.peakRight = snapshot.peak_r;
        output.rmsLeft = snapshot.rms_l;
        output.rmsRight = snapshot.rms_r;
        output.clippingCount = snapshot.clipping_count;
        output.dcOffsetLeft = snapshot.dc_offset_l;
        output.dcOffsetRight = snapshot.dc_offset_r;
        output.phaseCorrelation = snapshot.phase_correlation;
        output.loudnessLufs = snapshot.loudness_lufs;
        for (size_t i = 0; i < 8; ++i) output.spectrumRms[i] = snapshot.spectrum_rms[i];
        return true;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
