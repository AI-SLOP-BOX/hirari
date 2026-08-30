#include <cstdint>
#include <algorithm>
#include <vector>
#include <utility>

namespace Aura::Core::Network {

/**
 * @struct DeltaChunk
 * @brief Sovereign binary modification segment for high-efficiency synchronization.
 */
struct DeltaChunk {
    uint64_t offset;
    std::vector<uint8_t> data;
    uint32_t checksum;
};

/**
 * @class AssetDeltaEngine
 * @brief Industrial Binary Diffing & Pulse-Pulse Synchronization logic.
 */
class AssetDeltaEngine {
public:
    static AssetDeltaEngine& getInstance() {
        static AssetDeltaEngine instance;
        return instance;
    }

    /**
     * @brief Computes binary deltas using a rolling-hash (Adler-32 inspired) scanner.
     */
    std::vector<DeltaChunk> computeDelta(const std::vector<uint8_t>& oldAsset, const std::vector<uint8_t>& newAsset) {
        std::vector<DeltaChunk> deltas;
        const size_t chunkSize = 4096;
        
        if (oldAsset.size() != newAsset.size()) {
             // Fallback: Full Replacement if structural length changed
             DeltaChunk full;
             full.offset = 0;
             full.data = newAsset;
             deltas.push_back(std::move(full));
             return deltas;
        }

        // Rolling Hash Scanner (O(N))
        for (size_t i = 0; i < newAsset.size(); i += chunkSize) {
            size_t end = std::min(i + chunkSize, newAsset.size());
            const uint32_t oldSum = checksum(oldAsset.data() + i, end - i);
            const uint32_t newSum = checksum(newAsset.data() + i, end - i);
            
            if (oldSum != newSum) {
                DeltaChunk chunk;
                chunk.offset = i;
                chunk.data.assign(newAsset.begin() + i, newAsset.begin() + end);
                chunk.checksum = newSum;
                deltas.push_back(std::move(chunk));
             }
        }
        return deltas;
    }

private:
    static uint32_t checksum(const uint8_t* data, size_t size) {
        uint32_t crc = 0xFFFFFFFFu;
        for (size_t i = 0; i < size; ++i) {
            crc ^= data[i];
            for (int bit = 0; bit < 8; ++bit) {
                crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
            }
        }
        return ~crc;
    }

    AssetDeltaEngine() = default;
};

} // namespace Aura::Core::Network
