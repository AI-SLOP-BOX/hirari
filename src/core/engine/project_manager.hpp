#pragma once
#include <vector>
#include <string>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <system_error>
#include <atomic>
#include <mutex>
#include <sstream>
#include <cmath>
#include "../io/audio_decoder.hpp"
#include "timeline_system.hpp"
#include "project_version_store.hpp"

#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::Core::Engine {

/**
 * @class ProjectManager
 * @brief High-level Project Asset & Structure Management.
 */
class ProjectManager {
public:
    static ProjectManager& getInstance() {
        static ProjectManager instance;
        return instance;
    }

    bool consolidateAssets(const std::string& projectDir, TimelineSystem& timeline) {
        if (projectDir.empty()) return false;

        const std::filesystem::path projectPath(projectDir);
        std::error_code ec;
        if (!std::filesystem::is_directory(projectPath, ec) || ec) return false;

        // Copy the container while not holding any timeline lock during I/O.
        const auto tracks = timeline.getTracksSnapshot();
        for (const auto& track : tracks) {
            if (!track) return false;
            for (const auto& region : track->getRegions()) {
                if (region.path.empty()) return false;
                const std::filesystem::path source(region.path);
                if (!std::filesystem::is_regular_file(source, ec) || ec) return false;
            }
        }

        const std::filesystem::path assetDir = projectPath / "Assets";
        std::filesystem::create_directories(assetDir, ec);
        if (ec) return false;

        for (const auto& track : tracks) {
            for (const auto& region : track->getRegions()) {
                const auto source = std::filesystem::path(region.path);
                auto target = assetDir / source.filename();
                if (std::filesystem::exists(target, ec)) {
                    if (ec) return false;
                    if (std::filesystem::equivalent(source, target, ec)) {
                        if (ec) return false;
                        continue;
                    }
                    ec.clear();
                    target = assetDir / (source.stem().string() + "-" +
                                         stableAssetSuffix(source.string()) +
                                         source.extension().string());
                    uint32_t collision = 2;
                    while (std::filesystem::exists(target, ec)) {
                        if (ec) return false;
                        if (std::filesystem::equivalent(source, target, ec)) break;
                        if (ec) return false;
                        target = assetDir / (source.stem().string() + "-" +
                                             stableAssetSuffix(source.string()) + "-" +
                                             std::to_string(collision++) +
                                             source.extension().string());
                    }
                }
                ec.clear();
                if (!copyAssetAtomic(source, target, assetDir)) return false;
                if (!std::filesystem::is_regular_file(target, ec) || ec) return false;
            }
        }
        return true;
    }

    /**
     * @brief SAVE: Saves the current project state to a binary file with industrial precision and creative sovereignty.
     */
    void saveProject(const std::string& path, const std::vector<uint8_t>& projectData) {
        if (path.empty()) return;
        const std::filesystem::path destination(path);
        static std::atomic<uint64_t> saveSequence{0};
        const auto sequence = saveSequence.fetch_add(1, std::memory_order_relaxed);
#if defined(_WIN32)
        const auto temporary = destination.string() + ".tmp." + std::to_string(sequence);
#else
        const auto temporary = destination.string() + ".tmp." +
            std::to_string(static_cast<unsigned long>(::getpid())) + "." +
            std::to_string(sequence);
#endif

        std::error_code ec;
        std::filesystem::remove(temporary, ec);

        {
            std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
            if (!output) return;
            if (!projectData.empty()) {
                output.write(reinterpret_cast<const char*>(projectData.data()),
                             static_cast<std::streamsize>(projectData.size()));
            }
            output.flush();
            if (!output) return;
        }

        if (std::filesystem::file_size(temporary, ec) != projectData.size() || ec) {
            std::filesystem::remove(temporary, ec);
            return;
        }
#if !defined(_WIN32)
        const int fileFd = ::open(temporary.c_str(), O_RDONLY);
        if (fileFd < 0 || ::fsync(fileFd) != 0) {
            if (fileFd >= 0) ::close(fileFd);
            std::filesystem::remove(temporary, ec);
            return;
        }
        ::close(fileFd);
#endif
        std::filesystem::rename(temporary, destination, ec);
        if (ec) {
            std::filesystem::remove(temporary, ec);
            return;
        }
#if !defined(_WIN32)
        const auto parent = destination.parent_path().empty() ?
            std::filesystem::path(".") : destination.parent_path();
        const int dirFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
        if (dirFd >= 0) {
            (void)::fsync(dirFd);
            ::close(dirFd);
        }
#endif
    }

    void saveProject(const std::string& path, TimelineSystem& timeline) {
        std::vector<uint8_t> projectData;
        const auto tracks = timeline.getTracksSnapshot();
        projectData.reserve(sizeof(uint32_t) + tracks.size() * sizeof(uint32_t));
        const uint32_t count = static_cast<uint32_t>(tracks.size());
        const auto* countBytes = reinterpret_cast<const uint8_t*>(&count);
        projectData.insert(projectData.end(), countBytes, countBytes + sizeof(count));
        for (const auto& track : tracks) {
            if (!track) return;
            const uint32_t id = track->getId();
            const auto* idBytes = reinterpret_cast<const uint8_t*>(&id);
            projectData.insert(projectData.end(), idBytes, idBytes + sizeof(id));
        }
        saveProject(path, projectData);
    }

    // Store a reusable template without mutating the active project.
    bool saveTemplate(const std::string& path, const std::vector<uint8_t>& projectData) {
        if (path.empty() || projectData.empty()) return false;
        saveProject(path, projectData);
        std::error_code ec;
        return std::filesystem::is_regular_file(path, ec) && !ec &&
               std::filesystem::file_size(path, ec) == projectData.size() && !ec;
    }

    // Create a self-contained archive directory: project bytes plus every
    // referenced audio asset. Copying is atomic per file and never rewrites
    // the source project.
    bool archiveProject(const std::string& projectPath, const std::string& archiveDir,
                        TimelineSystem& timeline, const std::vector<uint8_t>& projectData) {
        if (projectPath.empty() || archiveDir.empty() || projectData.empty()) return false;
        std::error_code ec;
        const auto dir = std::filesystem::path(archiveDir);
        std::filesystem::create_directories(dir, ec);
        if (ec || !consolidateAssets(archiveDir, timeline)) return false;
        const auto target = dir / std::filesystem::path(projectPath).filename();
        saveProject(target.string(), projectData);
        if (!std::filesystem::is_regular_file(target, ec) || ec) return false;
        std::ofstream manifest(dir / "archive-manifest.tsv", std::ios::trunc);
        if (!manifest) return false;
        manifest << "source\tasset_directory\n";
        for (const auto& track : timeline.getTracksSnapshot()) {
            if (!track) return false;
            for (const auto& region : track->getRegions()) {
                if (region.path.empty()) return false;
                manifest << region.path << "\tAssets\n";
            }
        }
        manifest.flush();
        return static_cast<bool>(manifest);
    }

    bool saveVersion(const std::string& historyDir, const std::vector<uint8_t>& projectData,
                     ProjectVersionStore::Revision* revision = nullptr) {
        return ProjectVersionStore(historyDir).append(projectData, revision);
    }

private:
    static std::string stableAssetSuffix(const std::string& value) {
        uint64_t hash = 1469598103934665603ull;
        for (const unsigned char byte : value) { hash ^= byte; hash *= 1099511628211ull; }
        std::ostringstream out; out << std::hex << (hash & 0xffffffffull); return out.str();
    }
    static bool copyAssetAtomic(const std::filesystem::path& source,
                                const std::filesystem::path& target,
                                const std::filesystem::path& parent) {
        const auto temporary = target.string() + ".tmp-" + std::to_string(std::hash<std::string>{}(source.string()));
        std::error_code ec;
        { std::ifstream input(source, std::ios::binary); std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
          if (!input || !output) return false; output << input.rdbuf(); output.flush(); if (!output) return false; }
        std::filesystem::rename(temporary, target, ec);
        if (ec) { std::filesystem::remove(temporary, ec); return false; }
        (void)parent; return true;
    }

};

/**
 * @class BrowserPreviewPlayer
 * @brief Project-synchronized audio previewer.
 */
class BrowserPreviewPlayer {
public:
    void playPreview(const std::string& path, double projectSR, float projectBPM) {
        (void)projectBPM;
        stop();
        if (!std::isfinite(projectSR) || projectSR < 1000.0) return;
        auto decoded = ::Aura::Core::IO::AudioDecoderManager::getInstance().importFile(path);
        if (!decoded || decoded->getNumChannels() == 0 || decoded->getNumSamples() == 0) return;
        m_audio = std::move(decoded);
        m_sourceRate = ::Aura::Core::IO::AudioDecoderManager::getInstance().getLastSampleRate();
        m_targetRate = projectSR;
        m_position = 0.0;
        m_playing = true;
    }

    bool load(const std::string& path, double projectSR) {
        playPreview(path, projectSR, 120.0f);
        return m_audio != nullptr;
    }

    void stop() noexcept { m_playing = false; m_position = 0.0; }
    bool isPlaying() const noexcept { return m_playing; }
    double position() const noexcept { return m_position; }

    uint32_t render(float* left, float* right, uint32_t frames) noexcept {
        if (!left || !right || frames == 0 || !m_audio || !m_playing) return 0;
        const uint32_t channels = m_audio->getNumChannels();
        const uint32_t total = m_audio->getNumSamples();
        if (channels == 0 || total == 0) return 0;
        const double ratio = m_sourceRate / std::max(1000.0, m_targetRate);
        uint32_t rendered = 0;
        for (; rendered < frames && m_position < total; ++rendered) {
            const uint32_t index = static_cast<uint32_t>(m_position);
            const uint32_t next = std::min(index + 1, total - 1);
            const float frac = static_cast<float>(m_position - index);
            const float l0 = m_audio->getReadPointer(0)[index];
            const float l1 = m_audio->getReadPointer(0)[next];
            const float r0 = m_audio->getReadPointer(std::min<uint32_t>(1, channels - 1))[index];
            const float r1 = m_audio->getReadPointer(std::min<uint32_t>(1, channels - 1))[next];
            left[rendered] = std::isfinite(l0 + (l1 - l0) * frac) ? l0 + (l1 - l0) * frac : 0.0f;
            right[rendered] = std::isfinite(r0 + (r1 - r0) * frac) ? r0 + (r1 - r0) * frac : 0.0f;
            m_position += ratio;
        }
        if (m_position >= total) m_playing = false;
        return rendered;
    }

private:
    static bool copyAssetAtomic(const std::filesystem::path& source,
                                const std::filesystem::path& target,
                                const std::filesystem::path& parent) {
        static std::atomic<uint64_t> sequence{0};
#if defined(_WIN32)
        const auto processToken = 0ull;
#else
        const auto processToken = static_cast<unsigned long long>(::getpid());
#endif
        const auto temporary = std::filesystem::path(
            target.string() + ".tmp-" +
            std::to_string(processToken) + "-" +
            std::to_string(sequence.fetch_add(1, std::memory_order_relaxed)));
        std::error_code ec;
        {
            std::ifstream input(source, std::ios::binary);
            std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
            if (!input || !output) return false;
            output << input.rdbuf();
            output.flush();
            if (!output) {
                std::filesystem::remove(temporary, ec);
                return false;
            }
        }
#if !defined(_WIN32)
        const int fileFd = ::open(temporary.c_str(), O_RDONLY | O_CLOEXEC);
        if (fileFd < 0 || ::fsync(fileFd) != 0) {
            if (fileFd >= 0) ::close(fileFd);
            std::filesystem::remove(temporary, ec);
            return false;
        }
        ::close(fileFd);
#endif
        std::filesystem::rename(temporary, target, ec);
        if (ec) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
#if !defined(_WIN32)
        const int directoryFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (directoryFd < 0 || ::fsync(directoryFd) != 0) {
            if (directoryFd >= 0) ::close(directoryFd);
            return false;
        }
        ::close(directoryFd);
#endif
        return true;
    }

    static std::string stableAssetSuffix(const std::string& value) {
        uint64_t hash = 1469598103934665603ull;
        for (const unsigned char byte : value) {
            hash ^= byte;
            hash *= 1099511628211ull;
        }
        std::ostringstream out;
        out << std::hex << (hash & 0xffffffffull);
        return out.str();
    }

    std::shared_ptr<::Aura::Core::AudioBuffer> m_audio;
    double m_sourceRate = 44100.0;
    double m_targetRate = 44100.0;
    double m_position = 0.0;
    bool m_playing = false;
};

} // namespace Aura::Core::Engine
