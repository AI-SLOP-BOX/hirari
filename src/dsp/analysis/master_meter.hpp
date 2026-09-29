#pragma once

#include <algorithm>
#include <atomic>
#include <cmath>
#include <vector>

#include "analysis_engine.hpp"
#include "goniometer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** Public meter contract backed by Rust peak, RMS and true-peak processing. */
class MasterMeter {
public:
    struct Data {
        float momentary, shortTerm, integrated, lra;
        float truePeakL, truePeakR;
    };

    struct MeterData {
        float peakL, peakR;
        float truePeakL, truePeakR;
        float rmsL, rmsR;
        float lufsShortTerm;
        float lufsIntegrated;
        float correlation;
        float balance;
        std::vector<float> spectrumData;
        Goniometer::Data gonioData;
    };

    explicit MasterMeter(double sampleRate = 44'100.0)
        : m_sampleRate(validSampleRate(sampleRate)),
          m_core(hirari_master_meter_create(m_sampleRate)) {}
    ~MasterMeter() { hirari_master_meter_destroy(m_core); }

    MasterMeter(const MasterMeter&) = delete;
    MasterMeter& operator=(const MasterMeter&) = delete;

    void prepareToPlay(double sampleRate, [[maybe_unused]] uint32_t blockSize) {
        m_sampleRate = validSampleRate(sampleRate);
        hirari_master_meter_prepare(m_core, m_sampleRate);
    }

    void reset() noexcept { hirari_master_meter_reset(m_core); }

    void process(const float* left, const float* right, uint32_t samples) {
        if (!left || !right || samples == 0 || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) return;
        hirari_master_meter_process(m_core, left, right, samples);
    }

    std::vector<float> getSpectrogramL() const {
        std::vector<float> bands(64);
        for (uint32_t band = 0; band < bands.size(); ++band) {
            bands[band] = hirari_master_meter_spectrum_band(m_core, 0, band);
        }
        return bands;
    }

    MeterData getLatestData() const {
        float stats[4] = {-70.0f, -70.0f, -70.0f, -100.0f};
        hirari_master_meter_analysis_stats(m_core, stats);
        Goniometer::Data gonio{};
        hirari_master_meter_goniometer(
            m_core, gonio.xyHistoryL.data(), gonio.xyHistoryR.data(),
            Goniometer::kHistorySize, &gonio.correlation, &gonio.balance);
        return {
            hirari_master_meter_get(m_core, 0), hirari_master_meter_get(m_core, 1),
            hirari_master_meter_get(m_core, 2), hirari_master_meter_get(m_core, 3),
            hirari_master_meter_get(m_core, 4), hirari_master_meter_get(m_core, 5),
            stats[0], stats[2],
            gonio.correlation, gonio.balance,
            getSpectrogramL(), gonio
        };
    }

private:
    static double validSampleRate(double sampleRate) {
        return std::isfinite(sampleRate) && sampleRate >= 8'000.0 && sampleRate <= 384'000.0
            ? sampleRate : 44'100.0;
    }

    double m_sampleRate;
    void* m_core = nullptr;
};

} // namespace Hirari::DSP::Analysis
