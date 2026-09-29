#pragma once

#include "../core/rust_ffi.hpp"
#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>

namespace Hirari::IO {

/** C++ ownership adapter for the Rust memory-mapped WAVE reader. */
class MMapAudioFile {
public:
    explicit MMapAudioFile(const std::string& path)
        : m_handle(hirari_mapped_audio_file_open(path.c_str())) {
        if (!m_handle) throw std::runtime_error("Could not open or parse WAVE file: " + path);
    }

    ~MMapAudioFile() { hirari_mapped_audio_file_destroy(m_handle); }

    MMapAudioFile(const MMapAudioFile&) = delete;
    MMapAudioFile& operator=(const MMapAudioFile&) = delete;
    MMapAudioFile(MMapAudioFile&&) = delete;
    MMapAudioFile& operator=(MMapAudioFile&&) = delete;

    float getSample(uint32_t channel, uint64_t sampleIndex) const noexcept {
        return hirari_mapped_audio_file_sample(m_handle, channel, sampleIndex);
    }

    uint64_t getNumSamples() const noexcept {
        return hirari_mapped_audio_file_frames(m_handle);
    }

    uint32_t getNumChannels() const noexcept {
        return hirari_mapped_audio_file_channels(m_handle);
    }

    uint32_t getSampleRate() const noexcept {
        return hirari_mapped_audio_file_sample_rate(m_handle);
    }

    bool isValid() const noexcept {
        return hirari_mapped_audio_file_is_valid(m_handle);
    }

    size_t mappedFileSize() const noexcept {
        return hirari_mapped_audio_file_size(m_handle);
    }

    const void* getData() const noexcept {
        return hirari_mapped_audio_file_data(m_handle);
    }

    bool refreshFileState() const noexcept {
        return hirari_mapped_audio_file_refresh(m_handle);
    }

    const void* rustHandle() const noexcept { return m_handle; }

private:
    void* m_handle = nullptr;
};

} // namespace Hirari::IO
