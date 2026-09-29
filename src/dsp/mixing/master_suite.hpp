#pragma once

#include "dsp/analysis/master_meter.hpp"
#include "core/audio_buffer.hpp"
#include "core/rust_ffi.hpp"
#include "dsp/iprocessor.hpp"

namespace Hirari::DSP::Mixing {

/** C++ host facade; the mastering signal path is owned by Rust. */
class MasterSuite final : public IProcessor {
public:
    explicit MasterSuite(double sampleRate = 44'100.0)
        : m_state(hirari_master_suite_create(sampleRate)) {}
    ~MasterSuite() override { hirari_master_suite_destroy(m_state); }

    MasterSuite(const MasterSuite&) = delete;
    MasterSuite& operator=(const MasterSuite&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_master_suite_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getWritePointer(1);
        hirari_master_suite_process(m_state, left, right, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_master_suite_reset(m_state); }

    Analysis::MasterMeter::MeterData getLatestMetrics() const {
        float stats[4] = {-70.0f, -70.0f, -70.0f, -100.0f};
        hirari_master_suite_meter_analysis_stats(m_state, stats);
        Analysis::Goniometer::Data gonio{};
        hirari_master_suite_meter_goniometer(
            m_state, gonio.xyHistoryL.data(), gonio.xyHistoryR.data(),
            Analysis::Goniometer::kHistorySize, &gonio.correlation, &gonio.balance);
        Analysis::MasterMeter::MeterData data{};
        data.peakL = hirari_master_suite_meter_value(m_state, 0);
        data.peakR = hirari_master_suite_meter_value(m_state, 1);
        data.truePeakL = hirari_master_suite_meter_value(m_state, 2);
        data.truePeakR = hirari_master_suite_meter_value(m_state, 3);
        data.rmsL = hirari_master_suite_meter_value(m_state, 4);
        data.rmsR = hirari_master_suite_meter_value(m_state, 5);
        data.lufsShortTerm = stats[0];
        data.lufsIntegrated = stats[2];
        data.correlation = gonio.correlation;
        data.balance = gonio.balance;
        data.spectrumData.resize(64);
        for (uint32_t band = 0; band < data.spectrumData.size(); ++band) {
            data.spectrumData[band] = hirari_master_suite_meter_spectrum_band(m_state, 0, band);
        }
        data.gonioData = gonio;
        return data;
    }

    void setAutoGain(bool enable, float targetLUFS = -14.0f) {
        hirari_master_suite_set_auto_gain(m_state, enable, targetLUFS);
    }
    void setMidSideEnabled(bool enable) noexcept {
        hirari_master_suite_set_mid_side(m_state, enable);
    }
    void setStereoWidth(float width) noexcept {
        hirari_master_suite_set_width(m_state, width);
    }
    void setDitherEnabled(bool enable) noexcept {
        hirari_master_suite_set_dither(m_state, enable);
    }
    float getMasterGain() const noexcept { return hirari_master_suite_gain(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Mixing
