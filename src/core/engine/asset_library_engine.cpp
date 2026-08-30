#include "asset_library_engine.hpp"
#include <algorithm>
#include <cstring>
#include <iostream>
#include "../../io/wav_loader_utils.hpp"

namespace Aura::Core::Engine {

void AssetLibraryEngine::addSearchPath(const std::string& path) {
    if (path.empty()) return;
    std::lock_guard<std::mutex> lock(m_mutex);
    if (std::find(m_searchPaths.begin(), m_searchPaths.end(), path) != m_searchPaths.end()) return;
    m_searchPaths.push_back(path);
}

void AssetLibraryEngine::startIndexing() {
    stopIndexing();
    std::vector<std::string> paths;
    {
        std::lock_guard<std::mutex> lock(m_mutex);
        paths = m_searchPaths;
        m_index.clear();
        m_previewCache.clear();
    }
    m_stopIndexing.store(false, std::memory_order_release);
    m_isIndexing.store(true, std::memory_order_release);
    m_indexThread = std::thread([this, paths = std::move(paths)]() {
        for (const auto& root : paths) {
            if (m_stopIndexing.load(std::memory_order_acquire)) break;
            std::error_code rootError;
            if (!std::filesystem::is_directory(root, rootError) || rootError) continue;
            std::filesystem::recursive_directory_iterator it(
                root, std::filesystem::directory_options::skip_permission_denied, rootError);
            const std::filesystem::recursive_directory_iterator end;
            for (; it != end && !m_stopIndexing.load(std::memory_order_acquire); it.increment(rootError)) {
                if (rootError) { rootError.clear(); continue; }
                std::error_code fileError;
                if (!it->is_regular_file(fileError) || fileError) continue;
                const auto extension = it->path().extension().string();
                if (extension != ".wav" && extension != ".WAV") continue;
                const auto path = it->path().string();
                const auto size = it->file_size(fileError);
                if (fileError || path.size() >= sizeof(AssetMetadata::path)) continue;
                uint64_t id = 1469598103934665603ull;
                for (const unsigned char byte : path) { id ^= byte; id *= 1099511628211ull; }
                id ^= static_cast<uint64_t>(size);
                id *= 1099511628211ull;
                AssetMetadata asset{};
                asset.id = id == 0 ? 1 : id;
                asset.contentHash = id;
                std::strncpy(asset.path, path.c_str(), sizeof(asset.path) - 1);
                const auto name = it->path().stem().string();
                std::strncpy(asset.name, name.c_str(), sizeof(asset.name) - 1);
                asset.durationSeconds = 0.0f;
                // Header and channel metadata are collected during indexing
                // so the browser can display useful information without
                // decoding the asset again when it is selected.
                if (size <= 128ull * 1024ull * 1024ull) {
                    const auto decoded = ::Aura::IO::WavLoader::loadDiagnostic(path, false);
                    if (decoded.ok) {
                        asset.sampleRate = decoded.info.sampleRate;
                        asset.channelCount = decoded.info.numChannels;
                        asset.frameCount = decoded.info.numSamples;
                        asset.durationSeconds = asset.sampleRate > 0
                            ? static_cast<float>(static_cast<double>(asset.frameCount) / asset.sampleRate)
                            : 0.0f;
                    }
                }
                std::lock_guard<std::mutex> lock(m_mutex);
                m_index[asset.id] = asset;
            }
        }
        m_isIndexing.store(false, std::memory_order_release);
    });
}

void AssetLibraryEngine::stopIndexing() {
    m_stopIndexing.store(true, std::memory_order_release);
    if (m_indexThread.joinable()) m_indexThread.join();
    m_isIndexing.store(false, std::memory_order_release);
}

AssetLibraryEngine::~AssetLibraryEngine() { stopIndexing(); }

std::vector<AssetMetadata> AssetLibraryEngine::search(const SearchQuery& query) {
    std::lock_guard<std::mutex> lock(m_mutex);
    std::vector<AssetMetadata> results;

    for (const auto& pair : m_index) {
        const auto& asset = pair.second;
        bool match = true;

        if (!query.text.empty() && std::string(asset.name).find(query.text) == std::string::npos) match = false;
        // (Further tag-based and duration-based filtering logic)

        if (match) results.push_back(asset);
        if (results.size() >= 100) break; // Result cap for performance
    }
    return results;
}

bool AssetLibraryEngine::getPreview(uint64_t assetId, SpectralPreview& out) {
    std::lock_guard<std::mutex> lock(m_mutex);
    auto it = m_previewCache.find(assetId);
    if (it != m_previewCache.end()) {
        out = *it->second;
        return true;
    }
    return false;
}

void AssetLibraryEngine::generateSpectralPreview(const AssetMetadata& asset) {
    auto preview = std::make_shared<SpectralPreview>();
    preview->assetId = asset.id;
    const auto decoded = ::Aura::IO::WavLoader::loadDiagnostic(asset.path, false);
    if (!decoded.ok || decoded.channels.empty() || decoded.channels.front().empty()) return;
    const auto& samples = decoded.channels.front();
    constexpr size_t kWaveformBins = 512;
    constexpr size_t kSpectrumBins = 128;
    preview->waveform.resize(kWaveformBins, 0.0f);
    preview->spectrum.resize(kSpectrumBins, 0.0f);
    for (size_t bin = 0; bin < kWaveformBins; ++bin) {
        const size_t begin = (bin * samples.size()) / kWaveformBins;
        const size_t end = std::max(begin + 1, ((bin + 1) * samples.size()) / kWaveformBins);
        float peak = 0.0f;
        for (size_t i = begin; i < std::min(end, samples.size()); ++i)
            peak = std::max(peak, std::abs(std::isfinite(samples[i]) ? samples[i] : 0.0f));
        preview->waveform[bin] = peak;
    }
    // A bounded DFT is sufficient for a browser thumbnail and avoids
    // coupling the asset browser to the realtime FFT implementation.
    constexpr size_t kFftSize = 512;
    const size_t stride = std::max<size_t>(1, samples.size() / kFftSize);
    for (size_t bin = 0; bin < kSpectrumBins; ++bin) {
        const double frequency = static_cast<double>(bin) / kSpectrumBins;
        double real = 0.0, imag = 0.0;
        for (size_t n = 0; n < kFftSize; ++n) {
            const size_t index = std::min(n * stride, samples.size() - 1);
            const double phase = -2.0 * 3.141592653589793 * frequency * n;
            const double value = std::isfinite(samples[index]) ? samples[index] : 0.0;
            real += value * std::cos(phase);
            imag += value * std::sin(phase);
        }
        preview->spectrum[bin] = static_cast<float>(std::sqrt(real * real + imag * imag) / kFftSize);
    }
    std::lock_guard<std::mutex> lock(m_mutex);
    m_previewCache[asset.id] = preview;
}

} // namespace Aura::Core::Engine
