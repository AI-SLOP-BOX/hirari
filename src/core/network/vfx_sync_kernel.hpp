#include <mutex>
#include <condition_variable>

namespace Aura::Core::Network {

/**
 * @class VFXSyncKernel
 * @brief Logic for sample-accurate frame synchronization with external VFX engines.
 * Industrial Sovereignty: SHM-Ready Bitstream.
 */
class VFXSyncKernel {
public:
    static VFXSyncKernel& getInstance() {
        static VFXSyncKernel instance;
        return instance;
    }

    struct SyncPacket {
        uint64_t sampleIndex;
        double fractionalFrame; // Sub-sample offset for smooth interpolation
        uint32_t frameRate;
        uint32_t flags; // 0x1 = Wait for VFX Frame
    };

    /**
     * @brief Pushes current sync state to the Cross-Process Shared Memory (SHM) buffer.
     */
    void pushSync(uint64_t sampleIndex, double fractionalFrame = 0.0) {
        SyncPacket packet = { sampleIndex, fractionalFrame, 24, 0 };
        m_lastPacket.store(packet);
        
        // INDUSTRIAL: Signal the VFX Engine via SHM Atomic
        // (Mock: notification signal)
    }

    /**
     * @brief Bidirectional Handshake: Wait for external VFX engine to complete render.
     */
    void waitForVFX() {
        if (m_captureMode.load()) {
            std::unique_lock<std::mutex> lock(m_syncMutex);
            m_vfxReady.wait(lock, [this]{ return m_isVFXReady.load(); });
            m_isVFXReady.store(false);
        }
    }

    void signalVFXReady() {
        m_isVFXReady.store(true);
        m_vfxReady.notify_one();
    }

    void setCaptureMode(bool enabled) { m_captureMode.store(enabled); }

private:
    VFXSyncKernel() : m_lastPacket({0, 0, 0, 0}) {}
    
    std::atomic<SyncPacket> m_lastPacket;
    std::atomic<bool> m_captureMode{false};
    std::atomic<bool> m_isVFXReady{false};
    
    std::mutex m_syncMutex;
    std::condition_variable m_vfxReady;
};

} // namespace Aura::Core::Network
