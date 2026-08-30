#pragma once
#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <mutex>
#include <map>
#include <thread>
#include <filesystem>
#include "../diagnostics/forensic_kernel.hpp"
#include "../composition/harmonic_context_tracker.hpp"

namespace Aura::Core::Engine {

/**
 * @struct AssetMetadata
 * @brief Industrial Metadata with Sovereign License Tokenization.
 */
struct AssetMetadata {
    uint64_t id;
    char path[512];
    char name[128];
    char tags[256];
    char licenseToken[128]; // --- PHASE 84: LICENSE SOVEREIGNTY ---
    float durationSeconds;
    uint32_t sampleRate = 0;
    uint32_t channelCount = 0;
    uint64_t frameCount = 0;
    uint64_t contentHash = 0;
};

struct SearchQuery {
    std::string text;
};

struct SpectralPreview {
    uint64_t assetId;
    std::vector<float> waveform;
    std::vector<float> spectrum;
};

/**
 * @class AssetLibraryEngine
 * @brief Industrial Singularity Engine for Aura Studio Pro.
 * Implements autonomous license sovereignty and contextual caching.
 */
class AssetLibraryEngine {
public:
    static AssetLibraryEngine& getInstance() {
        static AssetLibraryEngine instance;
        return instance;
    }

    void addSearchPath(const std::string& path);
    void startIndexing();
    void stopIndexing();
    std::vector<AssetMetadata> search(const SearchQuery& query);
    bool getPreview(uint64_t assetId, SpectralPreview& out);
    void generateSpectralPreview(const AssetMetadata& asset);

    ~AssetLibraryEngine();

    void updateAssetSovereignty();
    bool validateLicense(uint64_t assetId);

private:
    AssetLibraryEngine() {
        m_isIndexing.store(false);
    }

    mutable std::mutex m_mutex;
    std::vector<std::string> m_searchPaths;
    std::atomic<bool> m_isIndexing;
    std::map<uint64_t, AssetMetadata> m_index;
    std::map<uint64_t, std::shared_ptr<SpectralPreview>> m_previewCache;
    std::thread m_indexThread;
    std::atomic<bool> m_stopIndexing{false};
};

} // namespace Aura::Core::Engine
