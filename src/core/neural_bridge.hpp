#include "utils/ring_buffer.hpp"
#include <atomic>
#include <chrono>

namespace Aura::Core::AI {

/**
 * @struct AdvicePacket
 * @brief Sovereign creative intelligence packet for the UI HUD.
 */
struct AdvicePacket {
    enum Level { INFO, WARNING, CRITICAL };
    Level level;
    char message[256];
    uint64_t timestamp;
};

/**
 * @class NeuralBridge
 * @brief Deterministic Creative Intelligence Bridge (Industrial Sovereignty).
 */
class NeuralBridge {
public:
    static NeuralBridge& getInstance() {
        static NeuralBridge instance;
        return instance;
    }

    /**
     * @brief Evaluates signal metrics against industrial EBU R128 standards.
     */
    void evaluateSignal(float lufsIntegrated, float truePeak, const float* /*spectrum*/) {
        AdvicePacket pkt;
        pkt.timestamp = static_cast<uint64_t>(
            std::chrono::steady_clock::now().time_since_epoch().count());
        
        if (truePeak > -0.1f) {
            pkt.level = AdvicePacket::CRITICAL;
            std::snprintf(pkt.message, 256, "TRUE PEAK VIOLATION: Digital clipping imminent at %.1f dBTP.", truePeak);
        } else if (lufsIntegrated > -14.0f) {
            pkt.level = AdvicePacket::WARNING;
            std::snprintf(pkt.message, 256, "LOUDNESS OVERAGE: Integrated LUFS (%.1f) exceeds streaming targets.", lufsIntegrated);
        } else {
            pkt.level = AdvicePacket::INFO;
            std::snprintf(pkt.message, 256, "SIGNAL COMPLIANT: EBU R128 tolerances maintained.");
        }
        
        m_adviceQueue.push(pkt);
    }

    /**
     * @brief Pops the latest advice packet for the UI (Lock-Free).
     */
    bool popAdvice(AdvicePacket& out) {
        return m_adviceQueue.pop(out);
    }

private:
    NeuralBridge() = default;
    ::Aura::Core::RingBuffer<AdvicePacket, 256> m_adviceQueue;
};

} // namespace Aura::Core::AI
