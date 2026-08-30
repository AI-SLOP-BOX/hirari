/* Aura Omni Ultimate - (c) 2026 Aura DAW Project */
#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <cmath>
#include <algorithm>
#include <string>
#include <cstring>
#include <cstddef>
#include <cstdint>
#include <mutex>

namespace Aura::Omni {
    inline constexpr std::size_t kSessionBytes = 128u << 20;
    inline constexpr std::size_t kMaxTracks = 256;
    // 1. COMPACT SESSION MEMORY (128MB)
    extern char S[kSessionBytes];
    extern std::atomic<int> P;
    
    // 2. SHARED ENGINE STATE
    extern std::atomic<uint64_t> EngineState;

    // 3. PHYSICAL CIRCUIT MODELING (Aliasing-free, denormal-safe saturation)
    inline float clean_saturate(float x, float& state) { 
        float sat = std::tanh(x);
        state += (sat - state) * 0.05f; 
        if (std::abs(state) < 1e-15f) state = 0.0f; // Denormal guard
        return state; 
    }
    
    inline float neural_layer(const float* input, const float* weights, int n) { 
        float r = 0; 
        for(int j = 0; j < n; ++j) r += input[j] * weights[j]; 
        return std::tanh(r); 
    }

    // 4. MODULATION SPRING PHYSICS
    struct Spring { 
        float x = 0, v = 0, k = 0.5, d = 0.1; 
        void update(float target) { 
            float f = -k * (x - target) - d * v; 
            v += f; 
            x += v; 
        } 
    };

    // 5. SPATIAL OBJECT PULLING (7.1.4 Constant-Power Distance Panning)
    struct SpatialObject { 
        float x = 0.0f, y = 0.0f, z = 0.0f; // Position coordinates in 3D (-1.0 to 1.0)
        
        void process(float input, float* output) { 
            // 7.1.4 Standard Layout coordinates:
            // L, R, C, LFE, Ls, Rs, Lb, Rb, Tfl, Tfr, Tbl, Tbr
            static constexpr float spk[12][3] = {
                {-1.f,  1.f, 0.f}, // L
                { 1.f,  1.f, 0.f}, // R
                { 0.f,  1.f, 0.f}, // C
                { 0.f,  0.f, 0.f}, // LFE (subwoofer)
                {-1.f, -0.5f, 0.f}, // Ls
                { 1.f, -0.5f, 0.f}, // Rs
                {-1.f, -1.f, 0.f}, // Lb
                { 1.f, -1.f, 0.f}, // Rb
                {-1.f,  1.f, 1.f}, // Tfl
                { 1.f,  1.f, 1.f}, // Tfr
                {-1.f, -1.f, 1.f}, // Tbl
                { 1.f, -1.f, 1.f}  // Tbr
            };
            
            float gains[12];
            float total_gain = 0.0f;
            
            for (int k = 0; k < 12; ++k) {
                if (k == 3) {
                    gains[k] = 0.5f; // LFE sub feed
                } else {
                    float dx = x - spk[k][0];
                    float dy = y - spk[k][1];
                    float dz = z - spk[k][2];
                    float dist_sq = dx*dx + dy*dy + dz*dz;
                    gains[k] = 1.0f / (std::sqrt(dist_sq) + 0.2f);
                }
                total_gain += gains[k] * gains[k];
            }
            
            // Normalize gains to preserve acoustic energy (constant power panning)
            float norm = 1.0f / std::sqrt(total_gain + 1e-9f);
            for (int k = 0; k < 12; ++k) {
                output[k] += input * gains[k] * norm;
            }
        } 
    };

    // 6. TRACK PROXY ARCHITECTURE (Smooth volume changes, clean saturation)
    struct TrackProxy { 
        uint32_t id; 
        float volume = 1, z[2] = {0}; 
        Spring volumeSmoother; 
        SpatialObject spatialObj;
        
        void process(float* l, float* r, int n, uint64_t /*ph*/) {
            // Apply volume smoother sample-by-sample inside the processing loop to prevent zipper noise
            for(int i = 0; i < n; ++i) { 
                volumeSmoother.update(volume); 
                float g = volumeSmoother.x;
                l[i] = clean_saturate(l[i], z[0]) * g;
                r[i] = clean_saturate(r[i], z[1]) * g;
            }
        }
    };

    // 7. SESSION INTEGRITY CHECKSUM (Robust 64-bit FNV-1a Checksum)
    inline uint64_t calculate_session_checksum(uint64_t playhead, const float* data, int n) { 
        uint64_t h = 14695981039346656037ULL; // FNV offset basis
        h ^= playhead;
        h *= 1099511628211ULL; // FNV prime
        for(int i = 0; i < n; ++i) {
            uint32_t bits;
            std::memcpy(&bits, &data[i], sizeof(uint32_t));
            h ^= bits;
            h *= 1099511628211ULL;
        }
        return h; 
    }

    // 8. OMNI-ENGINE CORE
    class Engine {
    public:
        static Engine& i() { 
            static Engine instance; 
            return instance; 
        }
        std::array<TrackProxy*, kMaxTracks> tracks{};
        std::size_t track_count = 0;

        struct SessionBuffer {
            void* data = nullptr;
            std::size_t capacity = 0;
            std::size_t size = 0;
        };

        bool addTrack(TrackProxy* track) noexcept {
            if (!track || track_count >= tracks.size()) return false;
            tracks[track_count++] = track;
            return true;
        }

        bool save(void* dst, std::size_t capacity, std::size_t& written) const noexcept {
            written = 0;
            if (!dst || capacity < kSessionBytes) return false;
            std::lock_guard<std::mutex> lock(m_sessionMutex);
            std::memcpy(dst, S, kSessionBytes);
            written = kSessionBytes;
            return true;
        }

        bool load(const void* src, std::size_t size) noexcept {
            if (!src || size < kSessionBytes) return false;
            std::lock_guard<std::mutex> lock(m_sessionMutex);
            std::memcpy(S, src, kSessionBytes);
            return true;
        }

        // SAVE and LOAD use SessionBuffer so the operation carries its size
        // contract across FFI/CLI boundaries. Unknown operations are errors.
        bool execute(int op, void* data) noexcept {
            switch (op) {
                case 1:
                    EngineState.fetch_or(1ULL << 63, std::memory_order_acq_rel);
                    return true;
                case 2:
                    EngineState.fetch_and(~(1ULL << 63), std::memory_order_acq_rel);
                    return true;
                case 3: {
                    if (!data) return false;
                    auto* buffer = static_cast<SessionBuffer*>(data);
                    return save(buffer->data, buffer->capacity, buffer->size);
                }
                case 4: {
                    if (!data) return false;
                    const auto* buffer = static_cast<const SessionBuffer*>(data);
                    return load(buffer->data, buffer->size);
                }
                default:
                    return false;
            }
        }

    private:
        mutable std::mutex m_sessionMutex;
    };

    // 9. TELEMETRY METRICS
    struct Metrics { 
        float peak, rms; 
        uint64_t checksum; 
    };
}
