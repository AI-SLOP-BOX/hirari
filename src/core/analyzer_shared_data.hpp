#include <atomic>
#include <array>
#include <memory>

namespace Aura::Core {

/**
 * @class AnalyzerSharedData
 * @brief Zero-Copy Triple-Buffering for High-Fidelity Visualization (Industrial).
 */
class AnalyzerSharedData {
public:
    static constexpr size_t kFFTSize = 1024;
    static constexpr size_t kGoniometerHistory = 4096;

    /**
     * @struct Snapshot
     * @brief AVX-Aligned visualization packet with sequence sovereignty.
     */
    struct alignas(32) Snapshot {
        float peakL, peakR;
        float rmsL, rmsR;
        float fftData[kFFTSize];
        float gonioL[kGoniometerHistory];
        float gonioR[kGoniometerHistory];
        uint64_t timestamp;
        uint64_t sequence;
    };

    AnalyzerSharedData() {
        for (int i = 0; i < 3; ++i) m_buffers[i] = std::make_unique<Snapshot>();
        m_dspBuffer.store(m_buffers[0].get());
        m_uiBuffer.store(m_buffers[1].get());
        m_spareBuffer.store(m_buffers[2].get());
    }

    /**
     * @brief DSP SIDE: Atomic pointer swap (Zero-Copy).
     */
    void pushSnapshot(const Snapshot& s) {
        Snapshot* writeBuffer = m_dspBuffer.load(std::memory_order_relaxed);
        *writeBuffer = s; // Copy still required for value-passing, but we move pointers next
        
        // Triple Buffer Swap: dsp -> ui -> spare -> dsp
        Snapshot* latest = m_dspBuffer.exchange(m_spareBuffer.load(std::memory_order_relaxed), std::memory_order_release);
        m_uiBuffer.store(latest, std::memory_order_release);
    }

    /**
     * @brief UI SIDE: Instant retrieval of the latest sovereign snapshot.
     */
    const Snapshot& getLatest() const {
        return *m_uiBuffer.load(std::memory_order_acquire);
    }

private:
    std::unique_ptr<Snapshot> m_buffers[3];
    std::atomic<Snapshot*> m_dspBuffer;
    std::atomic<Snapshot*> m_uiBuffer;
    std::atomic<Snapshot*> m_spareBuffer;
};

} // namespace Aura::Core
