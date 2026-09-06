#pragma once

#include <string>
#include <memory>
#include <vector>
#include <map>
#include <algorithm>
#include <fstream>
#include <cstring>
#include <cctype>
#include <limits>
#include <mutex>
#include <chrono>
#include <filesystem>
#include <cerrno>
#if !defined(_WIN32)
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
#include <signal.h>
extern char** environ;
#endif
#include "../audio_buffer.hpp"

namespace Aura::Core::IO {

/**
 * @interface IAudioDecoder
 * @brief Abstract interface for high-performance audio file decoding.
 * Essential for VST/Sampler compatibility and project importing.
 */
class IAudioDecoder {
public:
    virtual ~IAudioDecoder() = default;
    virtual bool open(const std::string& path) = 0;
    virtual void decodeFull(AudioBuffer& out) = 0;
    virtual std::string getFormatName() const = 0;
    virtual double getSampleRate() const = 0;
};

/**
 * @class WavDecoder
 * @brief Built-in RIFF/WAV Decoder (32-bit float / 16-bit PCM).
 */
class WavDecoder : public IAudioDecoder {
public:
    // Decoding is an offline operation, but a malformed header must not be
    // able to turn a drag-and-drop into an unbounded allocation. Keep this
    // limit aligned with the checked persistence reader.
    static constexpr uint64_t kMaximumDecodedBytes = 512ull * 1024ull * 1024ull;

    bool open(const std::string& path) override {
        m_dataOffset = 0;
        m_dataSize = 0;
        m_channels = 0;
        m_sampleRate = 0;
        m_bitsPerSample = 0;
        m_format = 0;
        m_path = path;
        std::ifstream file(path, std::ios::binary);
        if (!file.is_open()) return false;

        char riff[4];
        file.read(riff, 4);
        const bool isRiff = std::strncmp(riff, "RIFF", 4) == 0;
        const bool isRf64 = std::strncmp(riff, "RF64", 4) == 0;
        if (!isRiff && !isRf64) return false;

        uint32_t riffSize = 0;
        file.read(reinterpret_cast<char*>(&riffSize), sizeof(riffSize));
        if (!file || (!isRf64 && riffSize < 4)) return false;
        char wave[4];
        file.read(wave, 4);
        if (std::strncmp(wave, "WAVE", 4) != 0) return false;

        bool hasFmt = false;
        uint64_t rf64DataSize = 0;
        bool hasDs64 = !isRf64;
        const uint64_t riffEnd = !isRf64
            ? 8ull + static_cast<uint64_t>(riffSize)
            : std::numeric_limits<uint64_t>::max();
        while (file) {
            const auto chunkStart = file.tellg();
            if (chunkStart < 0 || (!isRf64 && static_cast<uint64_t>(chunkStart) > riffEnd) ||
                (!isRf64 && riffEnd - static_cast<uint64_t>(chunkStart) < 8u)) return false;
            char chunkId[4];
            uint32_t chunkSize = 0;
            file.read(chunkId, 4);
            file.read(reinterpret_cast<char*>(&chunkSize), 4);
            if (!file) break;
            const uint64_t paddedChunkBytes = static_cast<uint64_t>(chunkSize) + (chunkSize & 1u);
            const uint64_t chunkEnd = static_cast<uint64_t>(chunkStart) + 8u + paddedChunkBytes;
            if (!isRf64 && (chunkEnd < static_cast<uint64_t>(chunkStart) || chunkEnd > riffEnd)) return false;

            if (std::strncmp(chunkId, "ds64", 4) == 0 && isRf64) {
                if (chunkSize < 28u) return false;
                uint64_t ds64RiffSize = 0;
                uint64_t ds64DataSize = 0;
                uint64_t sampleCount = 0;
                uint32_t tableLength = 0;
                file.read(reinterpret_cast<char*>(&ds64RiffSize), sizeof(ds64RiffSize));
                file.read(reinterpret_cast<char*>(&ds64DataSize), sizeof(ds64DataSize));
                file.read(reinterpret_cast<char*>(&sampleCount), sizeof(sampleCount));
                file.read(reinterpret_cast<char*>(&tableLength), sizeof(tableLength));
                const uint64_t tableBytes = static_cast<uint64_t>(tableLength) * 12u;
                if (!file || tableBytes > static_cast<uint64_t>(chunkSize) - 28u) return false;
                file.seekg(static_cast<std::streamoff>(chunkSize - 28u - tableBytes + (chunkSize & 1u)), std::ios::cur);
                rf64DataSize = ds64DataSize;
                hasDs64 = true;
            } else if (std::strncmp(chunkId, "fmt ", 4) == 0) {
                struct WavFormatHeader {
                    uint16_t audioFormat;
                    uint16_t numChannels;
                    uint32_t sampleRate;
                    uint32_t byteRate;
                    uint16_t blockAlign;
                    uint16_t bitsPerSample;
                } fmt;
                if (chunkSize < sizeof(fmt)) return false;
                file.read(reinterpret_cast<char*>(&fmt), sizeof(fmt));
                if (chunkSize > sizeof(fmt)) {
                    file.seekg(static_cast<std::streamoff>((chunkSize - sizeof(fmt)) + (chunkSize & 1u)), std::ios::cur);
                } else if (chunkSize & 1u) {
                    file.seekg(1, std::ios::cur);
                }
                const uint16_t bytesPerSample = static_cast<uint16_t>(fmt.bitsPerSample / 8u);
                if (!file || fmt.numChannels == 0 || fmt.numChannels > 32 || fmt.sampleRate == 0 ||
                    fmt.sampleRate > 384000 || bytesPerSample == 0 ||
                    fmt.blockAlign != static_cast<uint16_t>(fmt.numChannels * bytesPerSample) ||
                    fmt.byteRate != fmt.sampleRate * fmt.blockAlign ||
                    !((fmt.audioFormat == 1 && (fmt.bitsPerSample == 16 || fmt.bitsPerSample == 24 || fmt.bitsPerSample == 32)) ||
                      (fmt.audioFormat == 3 && fmt.bitsPerSample == 32))) return false;
                m_channels = fmt.numChannels;
                m_sampleRate = fmt.sampleRate;
                m_bitsPerSample = fmt.bitsPerSample;
                m_format = fmt.audioFormat;
                hasFmt = true;
            } else if (std::strncmp(chunkId, "data", 4) == 0) {
                m_dataOffset = file.tellg();
                if (isRf64 && chunkSize == 0xFFFFFFFFu) {
                    if (!hasDs64 || rf64DataSize == 0) return false;
                    m_dataSize = rf64DataSize;
                } else {
                    m_dataSize = chunkSize;
                }
                file.seekg(0, std::ios::end);
                const auto fileSize = file.tellg();
                if (m_dataOffset < 0 || fileSize < m_dataOffset ||
                    static_cast<uint64_t>(fileSize - m_dataOffset) < m_dataSize ||
                    m_dataSize > kMaximumDecodedBytes) return false;
                break;
            } else {
                if (paddedChunkBytes > static_cast<uint64_t>(std::numeric_limits<std::streamoff>::max())) return false;
                file.seekg(static_cast<std::streamoff>(paddedChunkBytes), std::ios::cur);
                if (!file) return false;
            }
        }

        return hasFmt && m_dataOffset > 0 && (!isRf64 || hasDs64);
    }

    void decodeFull(AudioBuffer& out) override {
        std::ifstream file(m_path, std::ios::binary);
        if (!file.is_open() || m_dataOffset == 0) return;

        file.seekg(m_dataOffset);

        uint32_t bytesPerSample = m_bitsPerSample / 8;
        if (bytesPerSample == 0 || m_channels == 0) return;
        const uint64_t frameBytes = static_cast<uint64_t>(bytesPerSample) * m_channels;
        if (frameBytes == 0) return;
        if (m_dataSize % frameBytes != 0) return;
        const uint64_t frames = m_dataSize / frameBytes;
        if (frames == 0 || frames > std::numeric_limits<uint32_t>::max()) return;
        uint32_t totalSamples = static_cast<uint32_t>(frames);

        // The encoded payload limit alone is not sufficient: 512 MB of
        // interleaved 16-bit stereo expands to more than 2 GB of planar
        // float output. Reject that expansion before AudioBuffer::resize so
        // malformed or merely oversized assets cannot trigger an OOM.
        if (frames > std::numeric_limits<uint64_t>::max() / m_channels) return;
        const uint64_t outputSamples = frames * m_channels;
        if (outputSamples > std::numeric_limits<uint64_t>::max() / sizeof(float)) return;
        const uint64_t outputBytes = outputSamples * sizeof(float);
        if (outputBytes > kMaximumDecodedBytes) return;

        out.resize(m_channels, totalSamples);

        if (m_dataSize > kMaximumDecodedBytes) return;
        if (frameBytes > static_cast<uint64_t>(std::numeric_limits<size_t>::max())) return;

        // Decode in bounded chunks. The previous implementation allocated a
        // second buffer as large as the complete take in addition to the
        // output channels, which made otherwise valid 512 MB files spike to
        // roughly 1 GB before returning any audio.
        constexpr uint32_t kChunkFrames = 16384;
        const uint64_t chunkBytes64 = frameBytes * kChunkFrames;
        if (chunkBytes64 == 0 || chunkBytes64 > static_cast<uint64_t>(std::numeric_limits<size_t>::max()) ||
            chunkBytes64 > static_cast<uint64_t>(std::numeric_limits<std::streamsize>::max())) return;
        std::vector<char> buffer(static_cast<size_t>(chunkBytes64));
        auto decode_sample = [&](const uint8_t* source) -> float {
            if (m_format == 3 && m_bitsPerSample == 32) {
                float sample = 0.0f;
                std::memcpy(&sample, source, sizeof(sample));
                return std::isfinite(sample) ? sample : 0.0f;
            }
            if (m_bitsPerSample == 16) {
                int16_t sample = 0;
                std::memcpy(&sample, source, sizeof(sample));
                return static_cast<float>(sample) / 32768.0f;
            }
            if (m_bitsPerSample == 24) {
                const int32_t value = static_cast<int32_t>(
                    static_cast<uint32_t>(source[0]) |
                    (static_cast<uint32_t>(source[1]) << 8) |
                    (static_cast<uint32_t>(source[2]) << 16));
                const int32_t signed_value = (value & 0x00800000) != 0
                    ? value | static_cast<int32_t>(0xFF000000u) : value;
                return static_cast<float>(signed_value) / 8388608.0f;
            }
            int32_t sample = 0;
            std::memcpy(&sample, source, sizeof(sample));
            return static_cast<float>(sample) / 2147483648.0f;
        };
        uint64_t processed = 0;
        while (processed < frames) {
            const uint32_t current_frames = static_cast<uint32_t>(
                std::min<uint64_t>(kChunkFrames, frames - processed));
            const uint64_t current_bytes = static_cast<uint64_t>(current_frames) * frameBytes;
            file.read(buffer.data(), static_cast<std::streamsize>(current_bytes));
            if (!file) { out.resize(0, 0); return; }
            for (uint32_t frame = 0; frame < current_frames; ++frame) {
                const uint64_t output_frame = processed + frame;
                for (uint32_t channel = 0; channel < m_channels; ++channel) {
                    const auto* source = reinterpret_cast<const uint8_t*>(buffer.data()) +
                        static_cast<size_t>((static_cast<uint64_t>(frame) * m_channels + channel) * bytesPerSample);
                    out.getWritePointer(channel)[output_frame] = decode_sample(source);
                }
            }
            processed += current_frames;
        }
    }

    std::string getFormatName() const override { return "WAV"; }
    double getSampleRate() const override { return static_cast<double>(m_sampleRate); }
    uint32_t getNumChannels() const noexcept { return m_channels; }
    uint32_t getBitsPerSample() const noexcept { return m_bitsPerSample; }

private:
    std::string m_path;
    uint32_t m_channels = 0;
    uint32_t m_sampleRate = 0;
    uint32_t m_bitsPerSample = 0;
    uint16_t m_format = 0;
    std::streampos m_dataOffset = 0;
    uint64_t m_dataSize = 0;
};

/**
 * @class FfmpegAudioDecoder
 * @brief Bounded external decode adapter for formats not handled natively.
 *
 * FFmpeg is invoked with an argv vector (never through a shell), and its
 * output is written to a unique temporary WAV before the canonical WavDecoder
 * sees it.  This keeps format parsing, finite-sample sanitization, and memory
 * limits in one place while making a failed decode unable to replace a
 * project asset.
 */
class FfmpegAudioDecoder final : public IAudioDecoder {
public:
    explicit FfmpegAudioDecoder(std::string formatName)
        : m_formatName(std::move(formatName)) {}

    ~FfmpegAudioDecoder() override { removeTemporaryWav(); }

    bool open(const std::string& path) override {
        m_sourcePath = path;
        // Decoder instances may be reused; do not orphan the previous
        // converted file when a new source is opened.
        removeTemporaryWav();
        std::error_code error;
        if (!std::filesystem::is_regular_file(path, error)) return false;

#if defined(_WIN32)
        // The Windows launcher is intentionally kept explicit until the
        // process-handle/cancellation implementation is shared with the
        // encoder path. Do not silently pretend the format is native.
        return false;
#else
        try {
            const auto tempRoot = std::filesystem::temp_directory_path(error);
            if (error) return false;
            // Create a private 0700 directory atomically.  A predictable
            // output pathname under the shared temp root would let another
            // process pre-place a symlink before ffmpeg opens it with `-y`.
            std::string directoryTemplate = (tempRoot / "aura-import-XXXXXX").string();
            std::vector<char> directoryBuffer(directoryTemplate.begin(), directoryTemplate.end());
            directoryBuffer.push_back('\0');
            char* directory = ::mkdtemp(directoryBuffer.data());
            if (directory == nullptr) return false;
            const auto tempDirectory = std::filesystem::path(directory);
            m_temporaryDirectory = tempDirectory;
            const auto temp = tempDirectory / "decoded.wav";
            if (!decodeToWav(path, temp)) return false;
            // Register ownership before parsing so any subsequent failure is
            // covered by the same cleanup path.
            m_temporaryWav = temp;
            if (!m_delegate.open(temp.string())) {
                removeTemporaryWav();
                return false;
            }
            return true;
        } catch (...) {
            // A filesystem or allocation failure at this boundary is an
            // import failure, not an exception that should escape into the UI
            // or audio graph.
            removeTemporaryWav();
            return false;
        }
#endif
    }

    void decodeFull(AudioBuffer& out) override { m_delegate.decodeFull(out); }
    std::string getFormatName() const override { return m_formatName; }
    double getSampleRate() const override { return m_delegate.getSampleRate(); }

private:
#if !defined(_WIN32)
    static bool decodeToWav(const std::string& input, const std::filesystem::path& output) {
        posix_spawn_file_actions_t actions;
        if (posix_spawn_file_actions_init(&actions) != 0) return false;
        const int nullFd = ::open("/dev/null", O_WRONLY);
        if (nullFd < 0 ||
            posix_spawn_file_actions_adddup2(&actions, nullFd, STDERR_FILENO) != 0) {
            if (nullFd >= 0) close(nullFd);
            posix_spawn_file_actions_destroy(&actions);
            return false;
        }

        std::vector<std::string> arguments{
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-i", input, "-map", "0:a:0", "-vn", "-sn", "-dn",
            "-c:a", "pcm_f32le", "-fs",
            std::to_string(WavDecoder::kMaximumDecodedBytes + 44ull),
            "-f", "wav", output.string()
        };
        std::vector<char*> argv;
        argv.reserve(arguments.size() + 1);
        for (auto& argument : arguments) argv.push_back(argument.data());
        argv.push_back(nullptr);

        pid_t child = -1;
        const int spawnResult = posix_spawnp(&child, "ffmpeg", &actions, nullptr,
                                             argv.data(), environ);
        close(nullFd);
        posix_spawn_file_actions_destroy(&actions);
        if (spawnResult != 0) return false;

        int status = 0;
        constexpr auto kDecodeTimeout = std::chrono::seconds(120);
        const auto deadline = std::chrono::steady_clock::now() + kDecodeTimeout;
        for (;;) {
            const pid_t waited = waitpid(child, &status, WNOHANG);
            if (waited == child) break;
            if (waited < 0 && errno != EINTR) {
                kill(child, SIGTERM);
                waitpid(child, &status, 0);
                std::error_code error;
                std::filesystem::remove(output, error);
                return false;
            }
            if (std::chrono::steady_clock::now() >= deadline) {
                // A malformed or adversarial media stream must not hold the
                // DAW's import/control path forever. Reap the child after a
                // bounded graceful termination so no zombie survives.
                kill(child, SIGTERM);
                while (waitpid(child, &status, 0) < 0 && errno == EINTR) {}
                std::error_code error;
                std::filesystem::remove(output, error);
                return false;
            }
            // Avoid a hot spin while still checking often enough to keep the
            // timeout deterministic on long-running codec jobs.
            usleep(10'000);
        }
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            std::error_code error;
            std::filesystem::remove(output, error);
            return false;
        }
        std::error_code outputError;
        const bool regular = std::filesystem::is_regular_file(output, outputError);
        if (outputError || !regular) {
            std::filesystem::remove(output, outputError);
            return false;
        }
        const auto outputBytes = std::filesystem::file_size(output, outputError);
        if (outputError || outputBytes <= 44) {
            std::filesystem::remove(output, outputError);
            return false;
        }
        return true;
    }
#endif

    std::string m_sourcePath;
    std::string m_formatName;
    std::filesystem::path m_temporaryDirectory;
    std::filesystem::path m_temporaryWav;
    WavDecoder m_delegate;

    void removeTemporaryWav() noexcept {
        if (m_temporaryWav.empty() && m_temporaryDirectory.empty()) return;
        std::error_code error;
        if (!m_temporaryWav.empty()) std::filesystem::remove(m_temporaryWav, error);
        if (!m_temporaryDirectory.empty()) std::filesystem::remove(m_temporaryDirectory, error);
        m_temporaryWav.clear();
        m_temporaryDirectory.clear();
    }
};

/**
 * @class AudioDecoderManager
 * @brief Professional High-level Audio Import Engine.
 */
class AudioDecoderManager {
public:
    AudioDecoderManager() = default;
    enum class ImportStatus {
        Success,
        EmptyPath,
        FileNotFound,
        UnsupportedFormat,
        InvalidAudioFile,
        DecodeFailed
    };

    static AudioDecoderManager& getInstance() { static AudioDecoderManager i; return i; }

    ImportStatus getLastImportStatus() const {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_lastImportStatus;
    }
    double getLastSampleRate() const noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_lastSampleRate;
    }

    /**
     * @brief IMPORT: The central orchestrator for drag-and-drop audio.
     */
    std::shared_ptr<AudioBuffer> importFile(const std::string& path) {
        // The manager is a compatibility singleton used by multiple engine
        // sessions.  Keep the result pair (status, sample rate) coherent for
        // callers that import concurrently from waveform/render threads.
        std::unique_lock<std::mutex> lock(m_stateMutex);
        m_lastImportStatus = ImportStatus::Success;
        if (path.empty()) {
            m_lastImportStatus = ImportStatus::EmptyPath;
            return nullptr;
        }

        std::ifstream input(path, std::ios::binary);
        if (!input.is_open()) {
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

        std::string ext = path.substr(dot + 1);
        std::transform(ext.begin(), ext.end(), ext.begin(),
                       [](unsigned char c) { return static_cast<char>(std::tolower(c)); });

        std::unique_ptr<IAudioDecoder> decoder;
        if (ext == "wav" || ext == "rf64") decoder = std::make_unique<WavDecoder>();
        else if (ext == "mp3" || ext == "flac" || ext == "aif" ||
                 ext == "aiff" || ext == "m4a" || ext == "ogg" || ext == "aac") {
            decoder = std::make_unique<FfmpegAudioDecoder>(ext);
        } else {
            // Other formats remain explicitly unsupported until they have the
            // same bounded decoder and contract coverage.
            m_lastImportStatus = ImportStatus::UnsupportedFormat;
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
                    return audio;
                }
                m_cache.erase(cached);
            }
        }

        // Decoder open/decode may invoke an external process and can take
        // seconds (or up to the bounded FFmpeg timeout). Never hold the
        // manager mutex across that work: waveform requests, status polling,
        // and unrelated imports must remain responsive.
        lock.unlock();
        if (!decoder->open(path)) {
            lock.lock();
            m_lastImportStatus = ImportStatus::InvalidAudioFile;
            return nullptr;
        }

        auto out = std::make_shared<AudioBuffer>();
        decoder->decodeFull(*out);
        if (out->getNumChannels() == 0 || out->getNumSamples() == 0) {
            lock.lock();
            m_lastImportStatus = ImportStatus::DecodeFailed;
            return nullptr;
        }
        lock.lock();
        m_lastSampleRate = decoder->getSampleRate();
        m_lastImportStatus = ImportStatus::Success;
        if (!sizeError && !timeError) {
            m_cache[path] = CacheEntry{out, fileSize, modified, m_lastSampleRate};
            // Keep the control-plane cache bounded over long sessions with
            // many one-shot imports. Live buffers remain owned by regions;
            // only expired weak entries are discarded here.
            for (auto it = m_cache.begin(); it != m_cache.end();) {
                if (it->second.audio.expired()) it = m_cache.erase(it);
                else ++it;
            }
            while (m_cache.size() > 1024) m_cache.erase(m_cache.begin());
        }
        return out;
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

} // namespace Aura::Core::IO
