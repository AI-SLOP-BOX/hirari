#pragma once
#include <string>
#include <vector>
#include <map>
#include <mutex>
#include <filesystem>
#include <iostream>
#include <fstream>
#include <sstream>
#include <atomic>
#include <array>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::Core {

/**
 * @class AssetManager
 * @brief THE LIBRARIAN: Handles project files, consolidation, and path-relinking.
 * SOLVES: Point 5 of the audit. Prevents 'Missing File' dialogs by 
 * ensuring all recordings are tracked and consolidated to the project folder.
 */
class AssetManager {
public:
    static AssetManager& getInstance() { static AssetManager i; return i; }
    
    /**
     * @brief REGISTRATION: Tracks a new file (e.g., recorded audio).
     */
    std::string registerAsset(const std::string& path, bool copyToProject = true) {
        std::lock_guard<std::mutex> lock(m_mutex);
        
        std::filesystem::path p(path);
        std::error_code statusError;
        const auto status = std::filesystem::symlink_status(p, statusError);
        if (statusError || status.type() == std::filesystem::file_type::symlink ||
            !std::filesystem::is_regular_file(status)) return "";

        if (copyToProject && !m_projectFolder.empty()) {
            std::error_code ioError;
            auto dest = std::filesystem::path(m_projectFolder) / "Audio Files" / p.filename();
            if (!std::filesystem::create_directories(dest.parent_path(), ioError) && ioError) {
                return "";
            }
            
            try {
                if (std::filesystem::exists(dest)) {
                    if (std::filesystem::equivalent(p, dest, ioError)) {
                        if (ioError) return "";
                    } else {
                        ioError.clear();
                        // A source path is not an asset identity: recording a
                        // new take at the same path must never reuse the
                        // previous project copy merely because its basename
                        // stayed unchanged.  Derive the collision suffix from
                        // the bytes, with the path only as a deterministic
                        // fallback when the source cannot be read.
                        const auto suffix = contentAssetSuffix(p);
                        dest = dest.parent_path() /
                            (p.stem().string() + "-" + suffix + p.extension().string());
                        uint32_t collision = 2;
                        while (std::filesystem::exists(dest)) {
                            if (std::filesystem::equivalent(p, dest, ioError)) break;
                            if (ioError) return "";
                            dest = dest.parent_path() /
                                (p.stem().string() + "-" + suffix + "-" +
                                 std::to_string(collision++) + p.extension().string());
                        }
                    }
                }
                ioError.clear();
                if (!std::filesystem::exists(dest)) {
                    if (!copyFileAtomic(p, dest)) return "";
                }
                // The published filename is the identity inside the project;
                // using the original basename here would overwrite the
                // registry entry for a different same-named source.
                m_assets[dest.filename().string()] = dest.string();
                return dest.string();
            } catch(...) {
                std::cerr << "[AssetManager] Failed to copy asset to project folder." << std::endl;
                return "";
            }
        }
        
        m_assets[p.filename().string()] = p.string();
        return p.string();
    }
    
    /**
     * @brief RELATIVE PATH CONVERSION: Ensures project files (.aura) don't break 
     * when the project folder is moved.
     */
    std::string getRelativePath(const std::string& absolutePath) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_projectFolder.empty()) return absolutePath;
        
        std::filesystem::path p(absolutePath);
        std::filesystem::path root(m_projectFolder);
        
        try {
            auto rel = std::filesystem::relative(p, root);
            return rel.string();
        } catch(...) {
            return absolutePath;
        }
    }

    /**
     * @brief ABSOLUTE PATH RECOVERY: Rebuilds paths from a moved project folder.
     */
    std::string resolvePath(const std::string& relativePath) {
        if (m_projectFolder.empty() || relativePath.empty()) return relativePath;
        auto p = std::filesystem::path(m_projectFolder) / relativePath;
        return p.lexically_normal().string();
    }
    
    void setProjectFolder(const std::string& folder) { 
        m_projectFolder = std::filesystem::path(folder).parent_path().string(); 
        std::cout << "[AssetManager] Context Root: " << m_projectFolder << std::endl;
    }
    
private:
    static bool copyFileAtomic(const std::filesystem::path& source,
                               const std::filesystem::path& destination) {
        static std::atomic<uint64_t> sequence{0};
#if defined(_WIN32)
        const auto processToken = 0ull;
#else
        const auto processToken = static_cast<unsigned long long>(::getpid());
#endif
        const auto temporary = std::filesystem::path(
            destination.string() + ".tmp-" + std::to_string(processToken) + "-" +
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
        std::filesystem::rename(temporary, destination, ec);
        if (ec) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
#if !defined(_WIN32)
        const auto parent = destination.parent_path();
        const int directoryFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (directoryFd < 0 || ::fsync(directoryFd) != 0) {
            if (directoryFd >= 0) ::close(directoryFd);
            return false;
        }
        ::close(directoryFd);
#endif
        return true;
    }

    static std::string contentAssetSuffix(const std::filesystem::path& path) {
        uint64_t hash = 1469598103934665603ull;
        std::ifstream input(path, std::ios::binary);
        if (input) {
            std::array<char, 64 * 1024> buffer{};
            while (input) {
                input.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
                const auto count = input.gcount();
                for (std::streamsize i = 0; i < count; ++i) {
                    hash ^= static_cast<unsigned char>(buffer[static_cast<size_t>(i)]);
                    hash *= 1099511628211ull;
                }
            }
        } else {
            const auto fallback = path.string();
            for (const unsigned char byte : fallback) {
                hash ^= byte;
                hash *= 1099511628211ull;
            }
        }
        std::ostringstream out;
        out << std::hex << (hash & 0xffffffffull);
        return out.str();
    }

    std::string m_projectFolder;
    std::map<std::string, std::string> m_assets; // Map of filename -> absolute path
    std::mutex m_mutex;
};

} // namespace Aura::Core
