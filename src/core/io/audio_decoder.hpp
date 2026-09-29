#pragma once

#include "../audio_buffer.hpp"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <map>
#include <memory>
#include <mutex>
#include <limits>
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
void hirari_free_decoded_audio(float* samples, size_t sample_count);
}

namespace Hirari::Core::IO {

// Source-compatible offline WAV adapter for older utility callers. It does
// no parsing itself: the exact same Rust decoder supplies its channel data.
class WavDecoder {
public:
    bool open(const std::string& path) {
        m_buffer = AudioBuffer{};
        m_sampleRate = 0.0;
        m_bitsPerSample = 0;
        const auto dot = path.find_last_of('.');
        const auto separator = path.find_last_of("/\\");
        if (dot == std::string::npos || (separator != std::string::npos && dot < separator)) return false;
        std::string extension = path.substr(dot + 1);
        std::transform(extension.begin(), extension.end(), extension.begin(),
                       [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); });
        if (extension != "wav" && extension != "wave" && extension != "rf64") return false;
        const auto decoded = hirari_decode_audio_file(path.c_str());
        struct Guard {
            float* samples;
            size_t count;
            ~Guard() { hirari_free_decoded_audio(samples, count); }
        } guard{decoded.samples, decoded.sample_count};
        if (decoded.status != 0 || !decoded.samples || decoded.channels == 0 ||
            decoded.channels > 32 || decoded.sample_count == 0 ||
            decoded.sample_count % decoded.channels != 0 ||
            decoded.sample_count > (512ull * 1024ull * 1024ull) / sizeof(float) ||
            !std::isfinite(decoded.sample_rate) || decoded.sample_rate < 8000.0 ||
            decoded.sample_rate > 384000.0) return false;
        const auto frames = decoded.sample_count / decoded.channels;
        if (frames > std::numeric_limits<uint32_t>::max() ||
            !m_buffer.resize(decoded.channels, static_cast<uint32_t>(frames))) return false;
        if (!hirari_audio_buffer_deinterleave_interleaved(
                decoded.samples, decoded.sample_count, m_buffer.getArrayOfWritePointers(),
                decoded.channels, static_cast<uint32_t>(frames))) return false;
        m_sampleRate = decoded.sample_rate;
        m_bitsPerSample = decoded.bits_per_sample;
        return true;
    }

    void decodeFull(AudioBuffer& out) const {
        if (!out.resize(m_buffer.getNumChannels(), m_buffer.getNumSamples())) return;
        for (uint32_t channel = 0; channel < m_buffer.getNumChannels(); ++channel) {
            std::copy_n(m_buffer.getReadPointer(channel), m_buffer.getNumSamples(),
                        out.getWritePointer(channel));
        }
    }
    std::string getFormatName() const { return "WAV"; }
    double getSampleRate() const { return m_sampleRate; }
    uint32_t getNumChannels() const { return m_buffer.getNumChannels(); }
    uint32_t getBitsPerSample() const { return m_bitsPerSample; }

private:
    AudioBuffer m_buffer;
    double m_sampleRate = 0.0;
    uint32_t m_bitsPerSample = 0;
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

    ImportStatus getLastImportStatus() const {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_lastImportStatus;
    }

    double getLastSampleRate() const noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_lastSampleRate;
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
        const auto dot = path.find_last_of('.');
        const auto separator = path.find_last_of("/\\");
        if (dot == std::string::npos || (separator != std::string::npos && dot < separator) ||
            dot + 1 >= path.size()) {
            m_lastImportStatus = ImportStatus::UnsupportedFormat;
            return nullptr;
        }
        std::string extension = path.substr(dot + 1);
        std::transform(extension.begin(), extension.end(), extension.begin(),
                       [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); });
        if (extension != "wav" && extension != "wave" && extension != "rf64" &&
            extension != "mp3" && extension != "flac" && extension != "aif" &&
            extension != "aiff" && extension != "m4a" && extension != "ogg" &&
            extension != "aac") {
            m_lastImportStatus = ImportStatus::UnsupportedFormat;
            return nullptr;
        }

        // Rust decoding can be expensive. Do not block status queries or
        // unrelated import requests while Symphonia reads the media file.
        lock.unlock();
        const auto decoded = hirari_decode_audio_file(path.c_str());
        struct DecodedAudioGuard {
            float* samples;
            size_t count;
            ~DecodedAudioGuard() { hirari_free_decoded_audio(samples, count); }
        } decodedGuard{decoded.samples, decoded.sample_count};

        if (decoded.status != 0 || !decoded.samples || decoded.channels == 0 ||
            decoded.channels > 32 || decoded.sample_count == 0 ||
            decoded.sample_count % decoded.channels != 0 ||
            decoded.sample_count > (512ull * 1024ull * 1024ull) / sizeof(float) ||
            !std::isfinite(decoded.sample_rate) || decoded.sample_rate < 8000.0 ||
            decoded.sample_rate > 384000.0) {
            lock.lock();
            m_lastImportStatus = decoded.status == 3
                ? ImportStatus::InvalidAudioFile : ImportStatus::DecodeFailed;
            return nullptr;
        }

        const size_t frames = decoded.sample_count / decoded.channels;
        if (frames > std::numeric_limits<uint32_t>::max()) {
            lock.lock();
            m_lastImportStatus = ImportStatus::DecodeFailed;
            return nullptr;
        }
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
