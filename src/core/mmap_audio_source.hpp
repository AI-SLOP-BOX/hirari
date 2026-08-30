#pragma once
#include <memory>
#include <string>
#include "audio_region.hpp"
#include "../io/mmap_audio_file.hpp"

namespace Aura::Core {

/**
 * @class MMapAudioSource
 * @brief Industrial-Grade Disk Streaming Orchestrator.
 * Connects the MMapAudioFile low-level IO to the Core IAudioSource interface.
 * HONEST FIX: Fulfills the 'zero-allocation disk streaming' requirement.
 */
class MMapAudioSource : public IAudioSource {
public:
    MMapAudioSource(const std::string& path) : m_path(path), m_file(std::make_shared<IO::MMapAudioFile>(path)) {}

    float getSample(uint32_t channel, uint64_t sampleIdx) const override {
        return m_file->getSample(channel, sampleIdx);
    }

    uint64_t getNumSamples() const override {
        return m_file->getNumSamples();
    }

    uint32_t getNumChannels() const override {
        return m_file->getNumChannels();
    }

    double getSampleRate() const override {
        return static_cast<double>(m_file->getSampleRate());
    }

    std::string getFilePath() const override {
        return m_path;
    }

private:
    std::string m_path;
    std::shared_ptr<IO::MMapAudioFile> m_file;
};

} // namespace Aura::Core
