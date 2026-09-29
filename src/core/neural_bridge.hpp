#include "rust_ffi.hpp"
#include <cstddef>
#include <chrono>

namespace Hirari::Core::AI {

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
static_assert(offsetof(AdvicePacket, level) == 0);
static_assert(offsetof(AdvicePacket, message) == 4);
static_assert(offsetof(AdvicePacket, timestamp) == 264);
static_assert(sizeof(AdvicePacket) == 272);

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
        const uint64_t timestamp = static_cast<uint64_t>(
            std::chrono::steady_clock::now().time_since_epoch().count());
        (void)hirari_neural_advice_evaluate(lufsIntegrated, truePeak, timestamp);
    }

    /**
     * @brief Pops the latest advice packet for the UI (Lock-Free).
     */
    bool popAdvice(AdvicePacket& out) {
        return hirari_neural_advice_pop(&out);
    }

private:
    NeuralBridge() = default;
};

} // namespace Hirari::Core::AI
