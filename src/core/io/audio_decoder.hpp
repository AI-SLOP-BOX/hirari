#pragma once

#include "../audio_buffer.hpp"
#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>

// The native engine keeps this small adapter for its existing AudioBuffer
// ownership model. Parsing, codec selection, and sample conversion live in
// hirari-core-bridge/src/preview_audio_runtime.rs.
extern "C" {
struct HirariNativeDecodedAudio {
    float* samples;
    size_t sample_count;
    uint32_t channels;
    double sample_rate;
    uint32_t bits_per_sample;
    uint32_t status;
};
HirariNativeDecodedAudio hirari_decode_audio_file(const char* path);
HirariNativeDecodedAudio hirari_import_audio_file(const uint8_t* path, size_t path_size);
void hirari_free_decoded_audio(float* samples, size_t sample_count);
// Import cache policy (path validation, file identity, hit/miss, eviction)
// lives in hirari-core-bridge/src/audio_import_cache.rs. These exchange an
// opaque weak audio handle; the decoded buffers stay native.
int32_t hirari_audio_import_cache_acquire(const uint8_t* path, size_t path_size,
                                          double* sample_rate, void** audio_handle);
bool hirari_audio_import_cache_store(const uint8_t* path, size_t path_size,
                                     double sample_rate, void* weak_audio);
void* hirari_audio_import_weak_lock(void* weak_audio);
bool hirari_audio_import_weak_alive(void* weak_audio);
void hirari_audio_import_weak_release(void* weak_audio);
}

namespace Hirari::Core::IO {

class AudioDecoderManager {
public:
    static AudioDecoderManager& getInstance() {
        static AudioDecoderManager instance;
        return instance;
    }

    std::shared_ptr<AudioBuffer> importFile(const std::string& path,
                                            double* sampleRateOut = nullptr) {
        if (sampleRateOut) *sampleRateOut = 0.0;
        const auto* pathBytes = reinterpret_cast<const uint8_t*>(path.data());
        double cachedRate = 0.0;
        void* cachedHandle = nullptr;
        const int32_t outcome = hirari_audio_import_cache_acquire(
            pathBytes, path.size(), &cachedRate, &cachedHandle);
        if (outcome == 0) {
            std::shared_ptr<AudioBuffer> cached;
            if (auto* owned = static_cast<std::shared_ptr<AudioBuffer>*>(cachedHandle)) {
                cached = std::move(*owned);
                delete owned;
            }
            if (!cached || cached->getNumSamples() == 0 || cached->getNumChannels() == 0) {
                return nullptr;
            }
            if (sampleRateOut) *sampleRateOut = cachedRate;
            return cached;
        }
        // The cache only reports "needs decode" for a readable file; an empty
        // or missing path is a terminal rejection.
        if (outcome != 3) return nullptr;

        const auto decoded = hirari_import_audio_file(pathBytes, path.size());
        struct DecodedAudioGuard {
            float* samples;
            size_t count;
            ~DecodedAudioGuard() { hirari_free_decoded_audio(samples, count); }
        } decodedGuard{decoded.samples, decoded.sample_count};
        if (decoded.status != 0 || !decoded.samples || decoded.channels == 0) return nullptr;

        auto audio = std::make_shared<AudioBuffer>();
        try {
            const size_t frames = decoded.sample_count / decoded.channels;
            if (!audio->resize(decoded.channels, static_cast<uint32_t>(frames)) ||
                !hirari_audio_buffer_deinterleave_interleaved(
                    decoded.samples, decoded.sample_count, audio->getArrayOfWritePointers(),
                    decoded.channels, static_cast<uint32_t>(frames))) {
                return nullptr;
            }
        } catch (...) {
            return nullptr;
        }
        if (sampleRateOut) *sampleRateOut = decoded.sample_rate;
        // The cache takes ownership of the weak handle and releases it when
        // the entry is replaced, evicted, or refused.
        hirari_audio_import_cache_store(pathBytes, path.size(), decoded.sample_rate,
                                        new std::weak_ptr<AudioBuffer>(audio));
        return audio;
    }
};

} // namespace Hirari::Core::IO
