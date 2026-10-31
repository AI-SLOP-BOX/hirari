#pragma once

#include "../audio_buffer.hpp"
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <map>
#include <memory>
#include <mutex>
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
void* hirari_wav_decoder_create();
void hirari_wav_decoder_destroy(void* state);
bool hirari_wav_decoder_open(void* state, const uint8_t* path, size_t path_size);
bool hirari_wav_decoder_copy_to_channels(
    const void* state, float* const* channels, uint32_t channel_count, uint32_t frames);
uint32_t hirari_wav_decoder_frames(const void* state);
double hirari_wav_decoder_sample_rate(const void* state);
uint32_t hirari_wav_decoder_channels(const void* state);
}

namespace Hirari::Core::IO {

// Source-compatible offline WAV adapter for older utility callers. It does
// no parsing itself: the exact same Rust decoder supplies its channel data.
class WavDecoder {
public:
    WavDecoder() : m_state(hirari_wav_decoder_create()) {}
    ~WavDecoder() { hirari_wav_decoder_destroy(m_state); }
    WavDecoder(const WavDecoder&) = delete;
    WavDecoder& operator=(const WavDecoder&) = delete;
    WavDecoder(WavDecoder&& other) noexcept : m_state(other.m_state) { other.m_state = nullptr; }
    WavDecoder& operator=(WavDecoder&& other) noexcept {
        if (this != &other) {
            hirari_wav_decoder_destroy(m_state);
            m_state = other.m_state;
            other.m_state = nullptr;
        }
        return *this;
    }

    bool open(const std::string& path) {
        return m_state && hirari_wav_decoder_open(
            m_state, reinterpret_cast<const uint8_t*>(path.data()), path.size());
    }

    void decodeFull(AudioBuffer& out) const {
        const uint32_t channels = getNumChannels();
        const uint32_t frames = hirari_wav_decoder_frames(m_state);
        if (!out.resize(channels, frames)) return;
        (void)hirari_wav_decoder_copy_to_channels(
            m_state, out.getArrayOfWritePointers(), channels, frames);
    }
    double getSampleRate() const { return hirari_wav_decoder_sample_rate(m_state); }
    uint32_t getNumChannels() const { return hirari_wav_decoder_channels(m_state); }

private:
    void* m_state = nullptr;
};

class AudioDecoderManager {
public:
    enum class ImportStatus {
        Success,
        EmptyPath,
        FileNotFound,
        UnsupportedFormat,
        InvalidAudioFile,
        DecodeFailed
    };

    static AudioDecoderManager& getInstance() {
        static AudioDecoderManager instance;
        return instance;
    }

    std::shared_ptr<AudioBuffer> importFile(const std::string& path,
                                            double* sampleRateOut = nullptr) {
        if (sampleRateOut) *sampleRateOut = 0.0;
        std::unique_lock<std::mutex> lock(m_stateMutex);
        m_lastImportStatus = ImportStatus::Success;
        if (path.empty() || path.find('\0') != std::string::npos) {
            m_lastImportStatus = ImportStatus::EmptyPath;
            return nullptr;
        }
        std::error_code sizeError;
        std::error_code timeError;
        const auto fileSize = std::filesystem::file_size(path, sizeError);
        const auto modified = std::filesystem::last_write_time(path, timeError);
        if (!sizeError && !timeError) {
            auto cached = m_cache.find(path);
            if (cached != m_cache.end() && cached->second.fileSize == fileSize &&
                cached->second.modified == modified) {
                if (auto audio = cached->second.audio.lock()) {
                    m_lastSampleRate = cached->second.sampleRate;
                    if (sampleRateOut) *sampleRateOut = cached->second.sampleRate;
                    return audio;
                }
                m_cache.erase(cached);
            }
        }
        if (sizeError) {
            m_lastImportStatus = ImportStatus::FileNotFound;
            return nullptr;
        }
        // Rust decoding can be expensive. Do not block status queries or
        // unrelated import requests while Symphonia reads the media file.
        lock.unlock();
        const auto decoded = hirari_import_audio_file(
            reinterpret_cast<const uint8_t*>(path.data()), path.size());
        struct DecodedAudioGuard {
            float* samples;
            size_t count;
            ~DecodedAudioGuard() { hirari_free_decoded_audio(samples, count); }
        } decodedGuard{decoded.samples, decoded.sample_count};

        if (decoded.status != 0 || !decoded.samples) {
            lock.lock();
            m_lastImportStatus = decoded.status == 4 ? ImportStatus::UnsupportedFormat :
                (decoded.status == 3 ? ImportStatus::InvalidAudioFile : ImportStatus::DecodeFailed);
            return nullptr;
        }

        const size_t frames = decoded.sample_count / decoded.channels;
        auto audio = std::make_shared<AudioBuffer>();
        try {
            if (!audio->resize(decoded.channels, static_cast<uint32_t>(frames))) {
                lock.lock();
                m_lastImportStatus = ImportStatus::DecodeFailed;
                return nullptr;
            }
            if (!hirari_audio_buffer_deinterleave_interleaved(
                    decoded.samples, decoded.sample_count, audio->getArrayOfWritePointers(),
                    decoded.channels, static_cast<uint32_t>(frames))) {
                lock.lock();
                m_lastImportStatus = ImportStatus::DecodeFailed;
                return nullptr;
            }
        } catch (...) {
            lock.lock();
            m_lastImportStatus = ImportStatus::DecodeFailed;
            return nullptr;
        }

        lock.lock();
        m_lastSampleRate = decoded.sample_rate;
        if (sampleRateOut) *sampleRateOut = decoded.sample_rate;
        m_lastImportStatus = ImportStatus::Success;
        if (!timeError) {
            m_cache[path] = CacheEntry{audio, fileSize, modified, decoded.sample_rate};
            for (auto it = m_cache.begin(); it != m_cache.end();) {
                if (it->second.audio.expired()) it = m_cache.erase(it);
                else ++it;
            }
            while (m_cache.size() > 1024) m_cache.erase(m_cache.begin());
        }
        return audio;
    }

private:
    struct CacheEntry {
        std::weak_ptr<AudioBuffer> audio;
        uintmax_t fileSize = 0;
        std::filesystem::file_time_type modified{};
        double sampleRate = 44100.0;
    };

    mutable std::mutex m_stateMutex;
    ImportStatus m_lastImportStatus = ImportStatus::Success;
    double m_lastSampleRate = 44100.0;
    std::map<std::string, CacheEntry> m_cache;
};

} // namespace Hirari::Core::IO
