/* Aura DAW Ultimate - Atomic Source - (c) 2026 Aura DAW Project */
#include <atomic>
#include <cmath>
#include <algorithm>
#include <array>

namespace Aura {
    // ATOMIC MEMORY SLAB (128MB)
    alignas(64) static char SLAB[128<<20]; 
    static std::atomic<size_t> SLAB_PTR{0};

    void* alloc(size_t s) { 
        // 64-byte alignment to guarantee SIMD memory access safety
        size_t aligned_s = (s + 63) & ~size_t(63);
        size_t current = SLAB_PTR.load(std::memory_order_relaxed);
        while (true) {
            if (current + aligned_s > sizeof(SLAB)) return nullptr;
            if (SLAB_PTR.compare_exchange_weak(current, current + aligned_s, std::memory_order_release, std::memory_order_relaxed)) {
                return &SLAB[current];
            }
        }
    }

    struct Buf { 
        float* d[2]; 
        uint32_t s; 
        void clear() { 
            for(int c=0; c<2; ++c) {
                if (d[c]) {
                    std::fill_n(d[c], s, 0.0f);
                }
            }
        } 
    };

    struct Trk {
        uint32_t id; 
        float g = 1.0f;
        float freq = 440.0f;
        float phase = 0.0f;
        float z[2] = {0.0f, 0.0f}; 
        
        void proc_sum(float* l, float* r, uint32_t n) { 
            const float sampleRate = 44100.0f;
            const float dphase = freq * 6.283185307f / sampleRate;
            for(uint32_t i=0; i<n; ++i) { 
                float wave = std::sin(phase);
                phase += dphase;
                if (phase >= 6.283185307f) phase -= 6.283185307f;
                
                // Left channel processing with denormal guard
                z[0] += (wave - z[0]) * 0.5f;
                if (std::abs(z[0]) < 1e-15f) z[0] = 0.0f;
                l[i] += std::tanh(z[0] * 1.1f) * g;

                // Right channel processing with denormal guard
                z[1] += (wave - z[1]) * 0.5f;
                if (std::abs(z[1]) < 1e-15f) z[1] = 0.0f;
                r[i] += std::tanh(z[1] * 1.1f) * g;
            } 
        } 
    };

    // Track objects have a bounded lifetime and do not need the generic slab.
    // A reusable pool prevents project reloads from exhausting the 128MB bump area.
    alignas(64) static std::array<Trk, 256> TRACK_POOL{};
    static std::array<std::atomic<bool>, 256> TRACK_USED{};

    Trk* alloc_track(uint32_t& slot) {
        for (uint32_t i = 0; i < TRACK_USED.size(); ++i) {
            bool expected = false;
            if (TRACK_USED[i].compare_exchange_strong(expected, true,
                                                       std::memory_order_acq_rel,
                                                       std::memory_order_relaxed)) {
                slot = i;
                return &TRACK_POOL[i];
            }
        }
        return nullptr;
    }

    void release_track(uint32_t slot) {
        if (slot < TRACK_USED.size()) TRACK_USED[slot].store(false, std::memory_order_release);
    }

    struct Engine {
        static Engine& i() { static Engine instance; return instance; }
        std::atomic<uint64_t> ph{0}; 
        std::atomic<float> peakL{0.0f};
        std::atomic<float> peakR{0.0f};
        uint32_t dither_state = 123456789;

        // Lock-free track management (atomic loads/stores)
        std::atomic<Trk*> ts[256]; 
        std::atomic<uint32_t> nT{0};

        float next_dither() {
            dither_state = dither_state * 1664525 + 1013904223;
            // High-pass dither noise normalized for 16-bit DAC output quantization simulation
            return (static_cast<float>(dither_state) / 4294967296.0f - 0.5f) / 32768.0f;
        }

        void process(float* l, float* r, uint32_t n) {
            uint64_t current_ph = ph.load(std::memory_order_acquire);
            bool is_playing = (current_ph & (1ULL << 63)) != 0;
            if (!is_playing) {
                std::fill_n(l, n, 0.0f);
                std::fill_n(r, n, 0.0f);
                return;
            }

            // Clear destination buffers at the start of the block to support proper summing
            std::fill_n(l, n, 0.0f);
            std::fill_n(r, n, 0.0f);

            uint32_t tracks_count = nT.load(std::memory_order_acquire);
            for(uint32_t i=0; i<tracks_count; ++i) {
                Trk* t = ts[i].load(std::memory_order_acquire);
                if (t) {
                    t->proc_sum(l, r, n);
                }
            }

            // Apply 16-bit dither & compute peak levels with decay
            float localPeakL = 0.0f;
            float localPeakR = 0.0f;
            for (uint32_t i = 0; i < n; ++i) {
                localPeakL = std::max(localPeakL, std::abs(l[i]));
                localPeakR = std::max(localPeakR, std::abs(r[i]));
                l[i] += next_dither();
                r[i] += next_dither();
            }

            // Exponential peak meter decay
            float prevPeakL = peakL.load(std::memory_order_relaxed);
            while (localPeakL > prevPeakL || localPeakL < prevPeakL * 0.999f) {
                float target = (localPeakL > prevPeakL) ? localPeakL : prevPeakL * 0.999f;
                if (peakL.compare_exchange_weak(prevPeakL, target, std::memory_order_relaxed)) break;
            }
            float prevPeakR = peakR.load(std::memory_order_relaxed);
            while (localPeakR > prevPeakR || localPeakR < prevPeakR * 0.999f) {
                float target = (localPeakR > prevPeakR) ? localPeakR : prevPeakR * 0.999f;
                if (peakR.compare_exchange_weak(prevPeakR, target, std::memory_order_relaxed)) break;
            }

            // Safely increment playhead without corrupting the high play-state bit
            uint64_t next_ph;
            do {
                current_ph = ph.load(std::memory_order_acquire);
                is_playing = (current_ph & (1ULL << 63)) != 0;
                if (!is_playing) break;
                uint64_t playhead_val = current_ph & ~(1ULL << 63);
                next_ph = (1ULL << 63) | ((playhead_val + n) & ~(1ULL << 63));
            } while (!ph.compare_exchange_weak(current_ph, next_ph, std::memory_order_release, std::memory_order_relaxed));
        }

        void remove_all_tracks() {
            // Caller must stop transport before releasing pooled objects.
            ph.fetch_and(~(1ULL << 63), std::memory_order_acq_rel);
            const uint32_t count = nT.exchange(0, std::memory_order_acq_rel);
            for (uint32_t i = 0; i < count && i < 256; ++i) {
                ts[i].store(nullptr, std::memory_order_release);
                release_track(i);
            }
        }
    };

    extern "C" {
        void aura_cmd(uint64_t op, float v) { 
            if(op==1) Engine::i().ph.fetch_or(1ULL<<63); // PLAY
            if(op==2) Engine::i().ph.fetch_and(~(1ULL<<63)); // STOP
            if(op==3) {
                // Dynamically add track thread-safely via FFI command
                uint32_t idx = Engine::i().nT.load(std::memory_order_relaxed);
                if (idx < 256) {
                    uint32_t slot = 0;
                    Trk* t = alloc_track(slot);
                    if (t) {
                        t->id = slot;
                        t->g = v;
                        t->freq = 440.0f + idx * 110.0f;
                        t->phase = 0.0f;
                        t->z[0] = 0.0f; t->z[1] = 0.0f;
                        Engine::i().ts[idx].store(t, std::memory_order_release);
                        Engine::i().nT.store(idx + 1, std::memory_order_release);
                    }
                }
            }
            if(op==4) Engine::i().remove_all_tracks(); // CLEAR_TRACKS (transport must be stopped)
        }
        uint64_t aura_sync() { return Engine::i().ph.load(); }
    }
}
