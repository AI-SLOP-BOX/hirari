#pragma once

#include <string>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdexcept>
#include <cstring>
#include <cmath>
#include <limits>
#include <vector>
#include <atomic>
#include <cstdint>

namespace Aura::IO {

/**
 * @class MMapAudioFile
 * @brief Zero-Allocation Disk Streaming using Memory Mapping (mmap).
 * HONEST FIX: Replaced expensive 'load all to RAM' strategy with professional disk streaming.
 * Maps the entire file into the virtual address space, letting the OS handle the paging.
 */
class MMapAudioFile {
public:
    MMapAudioFile(const std::string& path) {
        m_fd = open(path.c_str(), O_RDONLY);
        if (m_fd < 0) throw std::runtime_error("Could not open audio file: " + path);
        
        struct stat st;
        if (fstat(m_fd, &st) != 0 || st.st_size < 12) {
            close(m_fd);
            m_fd = -1;
            throw std::runtime_error("Invalid or empty audio file: " + path);
        }
        if (!S_ISREG(st.st_mode) || static_cast<uintmax_t>(st.st_size) >
                                         std::numeric_limits<size_t>::max()) {
            close(m_fd);
            m_fd = -1;
            throw std::runtime_error("Unsupported WAVE file size: " + path);
        }
        m_fileSize = static_cast<size_t>(st.st_size);
        m_data = static_cast<uint8_t*>(mmap(nullptr, m_fileSize, PROT_READ, MAP_PRIVATE, m_fd, 0));
        if (m_data == MAP_FAILED) {
            m_data = nullptr;
            close(m_fd);
            m_fd = -1;
            throw std::runtime_error("Could not map audio file: " + path);
        }
        const auto fail = [&](const char* message) {
            munmap(m_data, m_fileSize);
            m_data = nullptr;
            close(m_fd);
            m_fd = -1;
            throw std::runtime_error(std::string(message) + ": " + path);
        };
        
        // --- HONEST FIX: ZERO-STRING RIFF PARSING ---
        const bool isRf64 = readU32(0) == 0x34364652u; // RF64
        if ((!isRf64 && readU32(0) != 0x46464952u) || readU32(8) != 0x45564157u) {
            fail("Not a RIFF/WAVE file");
        }
        uint64_t rf64DataSize = 0;
        bool hasDs64 = false;
        size_t offset = 12;
        while (offset <= m_fileSize && m_fileSize - offset >= 8) {
            const uint32_t chunkID = readU32(offset);
            const uint32_t size = readU32(offset + 4);
            size_t payload = static_cast<size_t>(size);
            if (isRf64 && size == std::numeric_limits<uint32_t>::max()) {
                if (!hasDs64 || rf64DataSize > m_fileSize - offset - 8 ||
                    rf64DataSize > std::numeric_limits<size_t>::max()) {
                    fail("Invalid RF64 ds64 data size");
                }
                payload = static_cast<size_t>(rf64DataSize);
            }
            if (payload > m_fileSize - offset - 8) {
                fail("Truncated RIFF chunk");
            }
            const size_t chunkData = offset + 8;

            if (chunkID == 0x34367364u) { // "ds64"
                if (!isRf64 || payload < 28) fail("Invalid RF64 ds64 chunk");
                const uint64_t riffSize = readU64(chunkData);
                (void)riffSize;
                rf64DataSize = readU64(chunkData + 8);
                hasDs64 = true;
            } else if (chunkID == 0x20746d66u) { // "fmt "
                if (payload < 16) fail("Invalid fmt chunk");
                m_formatTag = readU16(chunkData);
                m_numChannels = readU16(chunkData + 2);
                m_sampleRate = readU32(chunkData + 4);
                const uint32_t byteRate = readU32(chunkData + 8);
                const uint16_t blockAlign = readU16(chunkData + 12);
                m_bitDepth = readU16(chunkData + 14);
                const uint64_t expectedBlockAlign =
                    static_cast<uint64_t>(m_numChannels) * (m_bitDepth / 8u);
                const uint64_t expectedByteRate =
                    static_cast<uint64_t>(m_sampleRate) * expectedBlockAlign;
                if (m_numChannels == 0 || m_sampleRate == 0 ||
                    (m_formatTag != 1 && m_formatTag != 3) ||
                    (m_bitDepth != 16 && m_bitDepth != 24 && m_bitDepth != 32) ||
                    expectedBlockAlign > std::numeric_limits<uint16_t>::max() ||
                    blockAlign != expectedBlockAlign ||
                    expectedByteRate > std::numeric_limits<uint32_t>::max() ||
                    byteRate != expectedByteRate) {
                    fail("Unsupported WAVE format");
                }
            } else if (chunkID == 0x61746164) { // "data"
                m_dataOffset = chunkData;
                m_dataSize = (isRf64 && size == std::numeric_limits<uint32_t>::max())
                    ? rf64DataSize : payload;
            }
            const size_t aligned = payload + (payload & 1u);
            if (aligned > m_fileSize - offset - 8) {
                fail("Invalid RIFF chunk alignment");
            }
            offset += 8 + aligned;
        }

        if (m_dataSize == 0 || m_numChannels == 0 || m_bitDepth == 0) {
            fail("WAVE data chunk is missing or empty");
        }
        m_bytesPerSample = static_cast<uint32_t>(m_bitDepth / 8);
        m_byteStride = m_bytesPerSample * m_numChannels;
        if (m_byteStride == 0 || m_dataSize % m_byteStride != 0) {
            fail("WAVE data is not frame aligned");
        }
        if (m_dataOffset > m_fileSize || m_dataSize > m_fileSize - m_dataOffset) {
            fail("WAVE data is outside the mapped file");
        }
    }

    ~MMapAudioFile() {
        if (m_data && m_data != MAP_FAILED) munmap(m_data, m_fileSize);
        if (m_fd >= 0) close(m_fd);
    }

    /**
     * @brief Random Access Samples: Supports 16-bit, 24-bit, and 32-bit Float.
     */
    float getSample(uint32_t channel, uint64_t sampleIdx) const {
        if (!m_data || m_fileInvalidated.load(std::memory_order_acquire) ||
            channel >= m_numChannels || m_byteStride == 0 ||
            sampleIdx >= getNumSamples() ||
            sampleIdx > (std::numeric_limits<size_t>::max() - m_dataOffset) / m_byteStride) {
            return 0.0f;
        }
        const size_t byteOffset = m_dataOffset + static_cast<size_t>(sampleIdx) * m_byteStride +
                                  static_cast<size_t>(channel) * m_bytesPerSample;
        if (byteOffset > m_fileSize || m_bytesPerSample > m_fileSize - byteOffset) return 0.0f;
        const uint8_t* ptr = m_data + byteOffset;
        float value = 0.0f;
        
        if (m_formatTag == 3) { // IEEE FLOAT
            if (m_bitDepth != 32) return 0.0f;
            std::memcpy(&value, ptr, sizeof(value));
        } else if (m_bitDepth == 16) {
            const uint16_t raw = readU16(byteOffset);
            value = static_cast<int16_t>(raw) * 3.0517578125e-5f;
        } else if (m_bitDepth == 24) {
            int32_t val = static_cast<int32_t>(ptr[0]) |
                          (static_cast<int32_t>(ptr[1]) << 8) |
                          (static_cast<int32_t>(ptr[2]) << 16);
            if (val & 0x800000) val |= 0xFF000000;
            value = val * 1.1920928955078125e-7f;
        } else if (m_bitDepth == 32) {
            value = static_cast<int32_t>(readU32(byteOffset)) * 4.656612873077393e-10f;
        }
        return std::isfinite(value) ? value : 0.0f;
    }

    uint64_t getNumSamples() const { 
        return m_byteStride == 0 ? 0 : m_dataSize / m_byteStride;
    }
    
    uint32_t getNumChannels() const { return m_numChannels; }
    uint32_t getSampleRate() const { return m_sampleRate; }

    bool isValid() const noexcept {
        return m_data != nullptr && m_fd >= 0 &&
               !m_fileInvalidated.load(std::memory_order_acquire);
    }

    size_t mappedFileSize() const noexcept { return m_fileSize; }
    
    /**
     * @brief Direct buffer access: Returns pointer to start of audio data.
     */
    const void* getData() const {
        return (m_data && !m_fileInvalidated.load(std::memory_order_acquire))
            ? m_data + m_dataOffset : nullptr;
    }

    /** Rejects an externally replaced or truncated file before further reads. */
    bool refreshFileState() {
        if (m_fd < 0 || !m_data) return false;
        struct stat st{};
        if (fstat(m_fd, &st) != 0 || st.st_size < 0 || !S_ISREG(st.st_mode) ||
            static_cast<uintmax_t>(st.st_size) > std::numeric_limits<size_t>::max()) {
            m_fileInvalidated.store(true, std::memory_order_release);
            return false;
        }
        const size_t currentSize = static_cast<size_t>(st.st_size);
        if (currentSize != m_fileSize ||
            m_dataOffset > currentSize || m_dataSize > currentSize - m_dataOffset) {
            m_fileInvalidated.store(true, std::memory_order_release);
            return false;
        }
        return true;
    }

private:
    uint16_t readU16(size_t offset) const {
        uint16_t value = 0;
        std::memcpy(&value, m_data + offset, sizeof(value));
        return value;
    }

    uint32_t readU32(size_t offset) const {
        uint32_t value = 0;
        std::memcpy(&value, m_data + offset, sizeof(value));
        return value;
    }

    uint64_t readU64(size_t offset) const {
        uint64_t value = 0;
        std::memcpy(&value, m_data + offset, sizeof(value));
        return value;
    }

    int m_fd = -1;
    size_t m_fileSize = 0;
    uint8_t* m_data = nullptr;
    uint32_t m_dataOffset = 44;
    size_t m_dataSize = 0;
    uint16_t m_formatTag = 1; // PCM
    uint16_t m_numChannels = 2;
    uint16_t m_bitDepth = 16;
    uint32_t m_bytesPerSample = 2;
    uint32_t m_byteStride = 4;
    uint32_t m_sampleRate = 44100;
    std::atomic<bool> m_fileInvalidated{false};
};

} // namespace Aura::IO
