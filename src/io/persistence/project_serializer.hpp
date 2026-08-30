#pragma once

#include <string>
#include <vector>
#include <fstream>
#include <iostream>
#include <cstdint>
#include <filesystem>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::IO::Persistence {

/**
 * @brief ProjectSerializer: Handles saving and loading the entire project state.
 * Uses a human-readable format (JSON-like) for Logic Pro compatibility and power-user edits.
 * The compatibility trailer uses FNV-1a instead of XOR so common multi-byte
 * corruptions are not silently accepted.
 */
class ProjectSerializer {
public:
    ProjectSerializer() = default;

    /**
     * @brief Saves the project to the specified path.
     */
    bool saveProject(const std::string& path, const std::string& jsonData) {
        if (path.empty() || jsonData.empty()) return false;
        static std::atomic<uint64_t> sequence{0};
        const std::string temporary = path + ".tmp-" +
#if !defined(_WIN32)
            std::to_string(static_cast<unsigned long long>(::getpid())) + "-" +
#else
            std::string("0-") +
#endif
            std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) return false;

        // --- HONEST FIX: INTEGRITY GUARD (ECC-like) ---
        // Point 6: Every project gets a checksum to prevent loading 'ghost' or corrupted data.
        const uint64_t checksum = checksumFor(jsonData);
        
        file << jsonData << "\n--AURA_CRC:" << std::to_string(checksum);
        file.flush();
        if (!file) {
            file.close();
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
        file.close();
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
        ::close(fd);
#endif
        std::error_code renameError;
        std::filesystem::rename(temporary, path, renameError);
        if (renameError) {
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return false;
        }
#if !defined(_WIN32)
        const auto parent = std::filesystem::path(path).parent_path();
        const int dirFd = ::open((parent.empty() ? std::filesystem::path(".") : parent).c_str(), O_RDONLY | O_DIRECTORY);
        if (dirFd < 0 || ::fsync(dirFd) != 0) {
            if (dirFd >= 0) ::close(dirFd);
            return false;
        }
        ::close(dirFd);
#endif
        return true;
    }

    std::string loadProject(const std::string& path) {
        std::ifstream file(path);
        if (!file.is_open()) return "";

        std::string content((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
        
        const size_t crcPos = content.rfind("\n--AURA_CRC:");
        if (crcPos == std::string::npos) return ""; // Re-init needed or corrupted
        
        const std::string data = content.substr(0, crcPos);
        const auto markerSize = std::string("\n--AURA_CRC:").size();
        const std::string encoded = content.substr(crcPos + markerSize);
        uint64_t expected = 0;
        try {
            size_t parsed = 0;
            expected = std::stoull(encoded, &parsed, 10);
            if (parsed != encoded.size()) return "";
        } catch (...) {
            return "";
        }
        const uint64_t actual = checksumFor(data);
        
        if (actual != expected) {
            std::cerr << "[ECC Error] Project data corrupted!" << std::endl;
            return "";
        }
        return data;
    }

private:
    static uint64_t checksumFor(const std::string& data) noexcept {
        uint64_t hash = 14695981039346656037ull;
        for (const unsigned char byte : data) {
            hash ^= static_cast<uint64_t>(byte);
            hash *= 1099511628211ull;
        }
        return hash;
    }
};

} // namespace Aura::IO::Persistence
