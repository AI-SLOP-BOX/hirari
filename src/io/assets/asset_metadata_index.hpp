#pragma once

#include <string>
#include <vector>
#include <map>
#include <algorithm>
#include <unordered_map>
#include <unordered_set>
#include <sstream>

namespace Aura::IO::Assets {

/**
 * @brief AssetMetadata: Fast metadata for DAW assets (samples, presets).
 */
struct AssetMetadata {
    std::string name;
    std::string tags;
    double bpm;
    std::string key;
};

/**
 * @brief AssetMetadataIndex: High-performance indexing and search for production assets.
 */
class AssetMetadataIndex {
public:
    static AssetMetadataIndex& getInstance() {
        static AssetMetadataIndex instance;
        return instance;
    }

    /**
     * @brief InvertedIndex: Search for production assets by tag.
     */
    void registerAsset(const std::string& path, const std::string& tags) {
        m_index[path] = {path, tags, 0.0, ""};
        
        std::stringstream ss(tags);
        std::string token;
        while (std::getline(ss, token, ',')) {
            m_invertedIndex[token].insert(path);
        }
    }

    std::vector<std::string> search(const std::string& tagQuery) {
        std::vector<std::string> results;
        auto it = m_invertedIndex.find(tagQuery);
        if (it != m_invertedIndex.end()) {
            results.assign(it->second.begin(), it->second.end());
        }
        return results;
    }

private:
    AssetMetadataIndex() = default;
    std::unordered_map<std::string, AssetMetadata> m_index;
    std::unordered_map<std::string, std::unordered_set<std::string>> m_invertedIndex;
};

} // namespace Aura::IO::Assets
