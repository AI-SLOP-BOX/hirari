#include "mastering_orchestrator.hpp"
#include "../audio_buffer.hpp"

namespace Hirari::Core::Engine {

void MasteringOrchestrator::process(AudioBuffer& buffer) {
    if (buffer.isEmpty()) return;

    uint32_t channels = buffer.getNumChannels();
    uint32_t samples = buffer.getNumSamples();
    if (channels > 128 || m_rustProcessor == nullptr) return;

    std::array<float, 8> bandGains = m_cachedGains;
    {
        std::unique_lock<std::mutex> lock(m_mutex, std::try_to_lock);
        if (lock.owns_lock() && m_hasTargetProfile &&
            m_targetProfile.bins.size() == bandGains.size() &&
            m_currentProfile.bins.size() == bandGains.size()) {
            if (hirari_mastering_match_profile(m_currentProfile.bins.data(),
                                               m_targetProfile.bins.data(),
                                               bandGains.data(), bandGains.size()))
                m_cachedGains = bandGains;
        }
    }

    float* channelPointers[128];
    for (uint32_t channel = 0; channel < channels; ++channel) {
        channelPointers[channel] = buffer.getWritePointer(channel);
    }
    float momentary = m_metrics.momentary;
    if (hirari_mastering_processor_process(m_rustProcessor, channelPointers,
                                           channels, samples, bandGains.data(),
                                           &momentary))
        m_metrics.momentary = momentary;
}

void MasteringOrchestrator::analyzeSpectralProfile(const AudioBuffer& buffer) {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (buffer.isEmpty() || buffer.getNumChannels() > 128) return;
    uint32_t channels = buffer.getNumChannels();
    const float* channelPointers[128];
    for (uint32_t channel = 0; channel < channels; ++channel) {
        channelPointers[channel] = buffer.getReadPointer(channel);
    }
    float bins[8]{};
    if (hirari_mastering_analyze_profile(channelPointers, channels,
                                         buffer.getNumSamples(), bins))
        m_currentProfile.bins.assign(bins, bins + 8);
}

void MasteringOrchestrator::applyTargetProfile(const SpectralProfile& target) {
    std::lock_guard<std::mutex> lock(m_mutex);
    m_targetProfile = target;
    m_hasTargetProfile = true;
}

bool MasteringOrchestrator::exportDDP(const DDPConfig& config, const std::string& outputDir) {
    std::vector<HirariMasteringByteSlice> isrcCodes;
    isrcCodes.reserve(config.isrcCodes.size());
    for (const auto& code : config.isrcCodes)
        isrcCodes.push_back({reinterpret_cast<const uint8_t*>(code.data()), code.size()});
    return hirari_mastering_export_ddp(
        reinterpret_cast<const uint8_t*>(outputDir.data()), outputDir.size(),
        reinterpret_cast<const uint8_t*>(config.title.data()), config.title.size(),
        reinterpret_cast<const uint8_t*>(config.upc.data()), config.upc.size(),
        isrcCodes.data(), isrcCodes.size());
}

} // namespace Hirari::Core::Engine
