#pragma once

#include <string>
#include <vector>
#include <filesystem>
#include <iostream>
#include <sstream>
#include <cstdint>
#include <atomic>
#include <stdexcept>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::IO::Persistence {

/**
 * @brief ProjectCollector: Ensures project portability.
 * Scans project for all external assets and copies them into a unified folder.
 */
class ProjectCollector {
public:
    static ProjectCollector& getInstance() {
        static ProjectCollector instance;
        return instance;
    }

    /**
     * @brief "Collect All and Save" - Aggregates all used files.
     */
    bool collect(const std::string& projectDir, const std::vector<std::string>& assetPaths) {
        std::filesystem::path destDir = std::filesystem::path(projectDir) / "Assets";
        std::error_code setupError;
        std::filesystem::create_directories(destDir, setupError);
        if (setupError) return false;
        std::vector<std::filesystem::path> created;
        static std::atomic<uint64_t> copySequence{0};
#if defined(_WIN32)
        const auto processToken = 0ull;
#else
        const auto processToken = static_cast<unsigned long long>(::getpid());
#endif

        for (const auto& originalPath : assetPaths) {
            try {
                std::error_code sourceError;
                const auto sourceStatus = std::filesystem::symlink_status(originalPath, sourceError);
                if (sourceError || sourceStatus.type() == std::filesystem::file_type::symlink) {
                    throw std::runtime_error("asset source must not be a symbolic link");
                }
                std::filesystem::path src = std::filesystem::weakly_canonical(originalPath, sourceError);
                if (sourceError || !std::filesystem::is_regular_file(src)) {
                    throw std::runtime_error("asset source is not a readable regular file");
                }
                const auto name = src.filename().string();
                std::filesystem::path dest = destDir / name;
                if (std::filesystem::exists(dest) &&
                    std::filesystem::equivalent(src, dest)) continue;
                if (std::filesystem::exists(dest)) {
                    // Preserve both assets. The suffix is deterministic for
                    // repeatable project collection and never overwrites a
                    // different file with the same basename.
                    const auto suffix = stableSuffix(src.string());
                    dest = destDir / (src.stem().string() + "-" + suffix + src.extension().string());
                    uint32_t collision = 2;
                    while (std::filesystem::exists(dest) &&
                           !std::filesystem::equivalent(src, dest)) {
                        dest = destDir / (src.stem().string() + "-" + suffix + "-" +
                                          std::to_string(collision++) + src.extension().string());
                    }
                }
                const auto temporary = std::filesystem::path(
                    dest.string() + ".tmp-" +
                    std::to_string(processToken) + "-" +
                    std::to_string(copySequence.fetch_add(1, std::memory_order_relaxed)));
                std::filesystem::copy_file(src, temporary, std::filesystem::copy_options::none);
#if !defined(_WIN32)
                const int fileFd = ::open(temporary.c_str(), O_RDONLY | O_CLOEXEC);
                if (fileFd < 0 || ::fsync(fileFd) != 0) {
                    if (fileFd >= 0) ::close(fileFd);
                    std::error_code cleanupError;
                    std::filesystem::remove(temporary, cleanupError);
                    throw std::runtime_error("asset sync failed before publish");
                }
                ::close(fileFd);
#endif
                std::error_code publishError;
                std::filesystem::rename(temporary, dest, publishError);
                if (publishError) {
                    std::filesystem::remove(temporary);
                    throw std::runtime_error("asset publish failed: " + publishError.message());
                }
                // Register the published path before the directory durability
                // check so a failed fsync still rolls it back with the rest of
                // this collection transaction.
                created.push_back(dest);
#if !defined(_WIN32)
                const int directoryFd = ::open(destDir.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
                if (directoryFd < 0 || ::fsync(directoryFd) != 0) {
                    if (directoryFd >= 0) ::close(directoryFd);
                    throw std::runtime_error("asset directory sync failed after publish");
                }
                ::close(directoryFd);
#endif
                std::cout << "[Collector] Collected: " << dest.filename() << std::endl;
            } catch (const std::exception& e) {
                std::cerr << "[Collector Alert] Error copying: " << originalPath << " (" << e.what() << ")" << std::endl;
                std::error_code rollbackError;
                for (const auto& path : created) {
                    std::filesystem::remove(path, rollbackError);
                    rollbackError.clear();
                }
                return false;
            }
        }
        return true;
    }

private:
    static std::string stableSuffix(const std::string& value) {
        uint64_t hash = 1469598103934665603ull;
        for (const unsigned char byte : value) {
            hash ^= byte;
            hash *= 1099511628211ull;
        }
        std::ostringstream out;
        out << std::hex << (hash & 0xffffffffull);
        return out.str();
    }

    ProjectCollector() = default;
};

} // namespace Aura::IO::Persistence
