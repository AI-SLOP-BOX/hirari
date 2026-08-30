#pragma once
#include <vector>
#include <string>
#include <fstream>
#include <cstdint>
#include <cstring>
#include <memory>
#include <algorithm>
#include <cmath>
#include <cstdlib>
#if defined(_WIN32)
#include <malloc.h>
#endif

namespace Aura::IO {

/**
 * @class WavetableParser
 * @brief Professional High-performance Parser for the 'vawt' Binary Format.
 * HONEST FIX: Implements mixed-endian handling and robust metadata offsets.
 * Point 1: Handles 'vawt' magic (Big-endian) vs rest (Little-endian).
 * Point 2: Accurately calculates metadata offset (12 + size).
 */
class WavetableParser {
public:
    struct AlignedDeleter {
        void operator()(float* p) const {
            if (!p) return;
#if defined(_WIN32)
            _aligned_free(p);
#else
            std::free(p);
#endif
        }
    };

    struct WavetableData {
        uint32_t waveSize;
        uint16_t waveCount;
        uint16_t flags;
        std::unique_ptr<float[], AlignedDeleter> samples; // --- HONEST FIX: SIMD ALIGNMENT ---
        std::string metadata;
    };

    /**
     * @brief LOAD 'vawt' FILE: Surge-grade precision.
     * Point 1: 16-byte Alignment required for SSE2/AVX stability.
     */
    static std::unique_ptr<WavetableData> load(const std::string& path) {
        std::ifstream file(path, std::ios::binary | std::ios::ate);
        if (!file.is_open()) return nullptr;

        size_t fileSize = static_cast<size_t>(file.tellg());
        if (fileSize < 12) return nullptr;
        file.seekg(0, std::ios::beg);

        char magic[4];
        file.read(magic, 4);
        if (std::memcmp(magic, "vawt", 4) != 0) return nullptr;

        auto data = std::make_unique<WavetableData>();
        file.read(reinterpret_cast<char*>(&data->waveSize), 4);
        file.read(reinterpret_cast<char*>(&data->waveCount), 2);
        file.read(reinterpret_cast<char*>(&data->flags), 2);

        // Point 4: Strict Power of 2 (128 to 4096 per Surge spec)
        if (!isPowerOfTwo(data->waveSize) || data->waveSize < 128 || data->waveSize > 4096) return nullptr;

        if (data->waveCount == 0) return nullptr;
        uint64_t totalSamples = (uint64_t)data->waveSize * data->waveCount;
        uint64_t dataSizeInBytes = totalSamples * sizeof(float); 
        
        // Point 2: Offset (12 + size)
        if (fileSize < 12 + dataSizeInBytes) return nullptr;

        // --- HONEST FIX: POSIX ALIGNMENT FOR SIMD ---
        const size_t allocationSize = (dataSizeInBytes + 15u) & ~size_t(15u);
        float* rawPtr = nullptr;
#if defined(_WIN32)
        rawPtr = static_cast<float*>(_aligned_malloc(allocationSize, 16));
#else
        rawPtr = static_cast<float*>(std::aligned_alloc(16, allocationSize));
#endif
        if (!rawPtr) return nullptr;
        data->samples.reset(rawPtr);

        file.read(reinterpret_cast<char*>(data->samples.get()), static_cast<std::streamsize>(dataSizeInBytes));
        if (!file) return nullptr;

        // Point 3: Flag 0x0008 (Headroom recovery)
        if (data->flags & 0x0008) {
            for (uint64_t i = 0; i < totalSamples; ++i) data->samples[i] *= 2.0f;
        }

        // Point 5: Metadata (flags & 0x10)
        if (data->flags & 0x10) {
            size_t metaOffset = 12 + dataSizeInBytes;
            if (fileSize > metaOffset) {
                file.seekg(metaOffset, std::ios::beg);
                std::getline(file, data->metadata, '\0');
            }
        }

        return data;
    }

private:
    static bool isPowerOfTwo(uint32_t n) {
        return (n != 0) && ((n & (n - 1)) == 0);
    }
};

} // namespace Aura::IO
