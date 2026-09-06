#pragma once

#include <string>
#include <vector>
#include <unordered_map>
#include <shared_mutex>
#include <algorithm>
#include <cctype>
#include <mutex>

namespace Aura::IO::Assets {

/**
 * @brief AssetPatch: A high-quality Logic Pro-style instrument preset.
 */
struct AssetPatch {
    std::string name;
    std::string category;
    std::string filePath;
};

/**
 * @brief AudioAssetLibrary: Central manifest for Factory Content.
 */
class AudioAssetLibrary {
public:
    static AudioAssetLibrary& getInstance() {
        static AudioAssetLibrary instance;
        return instance;
    }

    /**
     * @brief Adds a new patch to the list.
     */
    void registerPatch(const std::string& name, const std::string& cat, const std::string& path) {
        if (name.empty() || cat.empty() || path.empty()) return;
        std::unique_lock lock(m_mutex);
        const auto existing = std::find_if(m_patches.begin(), m_patches.end(),
            [&](const AssetPatch& patch) { return patch.filePath == path; });
        if (existing != m_patches.end()) {
            // Rescans update metadata in place instead of duplicating the
            // same preset in the browser. Rebuild the category index because
            // a user can move a preset between categories.
            existing->name = name;
            existing->category = cat;
            rebuildCategoryIndexLocked();
            return;
        }
        m_patches.push_back(AssetPatch{name, cat, path});
        m_categoryMap[cat].push_back(m_patches.back());
    }

    /**
     * @brief Filters patches by category (e.g., "Drum", "Piano").
     */
    std::vector<AssetPatch> findByCategory(const std::string& cat) const {
        std::shared_lock lock(m_mutex);
        auto it = m_categoryMap.find(cat);
        if (it != m_categoryMap.end()) {
            return it->second;
        }
        return {};
    }

    /**
     * @brief Returns all patches in the library.
     */
    std::vector<AssetPatch> getAllPatches() const {
        std::shared_lock lock(m_mutex);
        return m_patches;
    }

    /**
     * @brief Searches names, categories, and paths without changing the
     *        canonical registration order.
     */
    std::vector<AssetPatch> search(const std::string& query,
                                   const std::string& category = {}) const {
        std::shared_lock lock(m_mutex);
        const auto needle = normalize(query);
        const auto categoryNeedle = normalize(category);
        std::vector<AssetPatch> result;
        for (const auto& patch : m_patches) {
            const bool categoryMatch = categoryNeedle.empty() ||
                normalize(patch.category).find(categoryNeedle) != std::string::npos;
            const bool queryMatch = needle.empty() ||
                normalize(patch.name).find(needle) != std::string::npos ||
                normalize(patch.category).find(needle) != std::string::npos ||
                normalize(patch.filePath).find(needle) != std::string::npos;
            if (categoryMatch && queryMatch) result.push_back(patch);
        }
        return result;
    }

    std::vector<std::string> categories() const {
        std::shared_lock lock(m_mutex);
        std::vector<std::string> result;
        result.reserve(m_categoryMap.size());
        for (const auto& [category, _] : m_categoryMap) result.push_back(category);
        // The backing index is an unordered_map. Sort the public catalog so
        // browser rows and serialized UI snapshots remain deterministic
        // across processes and standard-library implementations.
        std::sort(result.begin(), result.end());
        return result;
    }

private:
    AudioAssetLibrary() = default;

    static std::string normalize(const std::string& value) {
        std::string result;
        result.reserve(value.size());
        for (const unsigned char character : value) {
            if (std::isspace(character)) continue;
            result.push_back(static_cast<char>(std::tolower(character)));
        }
        return result;
    }

    void rebuildCategoryIndexLocked() {
        m_categoryMap.clear();
        for (const auto& patch : m_patches) {
            m_categoryMap[patch.category].push_back(patch);
        }
    }

    mutable std::shared_mutex m_mutex;
    std::vector<AssetPatch> m_patches;
    std::unordered_map<std::string, std::vector<AssetPatch>> m_categoryMap;
};

} // namespace Aura::IO::Assets
