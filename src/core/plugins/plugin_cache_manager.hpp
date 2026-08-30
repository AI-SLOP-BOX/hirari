#pragma once

#include <vector>
#include <string>
#include <map>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <thread>
#include <mutex>
#include <atomic>
#include <set>
#include <iomanip>
#include <fcntl.h>
#include <unistd.h>
#include <cstdlib>
#include <optional>
#include <array>
#include <sstream>
#include "plugin_admission.hpp"
#include "../AuraPluginSDK.hpp"

#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#endif

namespace Aura::Core::Plugins {

/**
 * @struct PluginMetadata
 * @brief INDUSTRIAL: High-fidelity metadata for AU/VST.
 */
struct PluginMetadata {
    std::string name;
    std::string path;
    std::string manufacturer;
    uint32_t type;
    uint32_t subtype;
    std::filesystem::file_time_type lastModified;
    uint64_t fingerprint = 0;
};

/**
 * @class PluginCacheManager
 * @brief INDUSTRIAL: High-Density AU Cache (Phase 38).
 * Implements SBF-v5 grade persistence and hardware-level discovery.
 */
class PluginCacheManager {
public:
    static constexpr uint32_t kCacheMagic = 0x41555241; // "AURA"
    static constexpr uint32_t kCacheVersion = 7;
    static PluginCacheManager& getInstance() { static PluginCacheManager instance; return instance; }

    // INDUSTRIAL: CRC32 Integrity Kernel (Sovereign Consistent)
    static uint32_t calculateCRC32(const uint8_t* data, size_t size) {
        uint32_t crc = 0xFFFFFFFF;
        for (size_t i = 0; i < size; ++i) {
            crc ^= data[i];
            for (int k = 0; k < 8; ++k) {
                crc = (crc >> 1) ^ (0xEDB88320 & (-(int32_t)(crc & 1)));
            }
        }
        return ~crc;
    }

    std::filesystem::path getCachePath() const {
        const char* home = std::getenv("HOME");
        std::filesystem::path auraDir = home ? std::filesystem::path(home) / ".aura" : std::filesystem::path("/tmp/.aura");
        std::filesystem::create_directories(auraDir);
        return auraDir / "plugin_cache_v7.bin";
    }

    static std::string cacheKey(const std::string& pluginPath) {
        if (pluginPath.empty()) return {};
        std::error_code ec;
        const auto path = std::filesystem::path(pluginPath);
        auto canonical = std::filesystem::weakly_canonical(path, ec);
        if (ec) {
            ec.clear();
            canonical = std::filesystem::absolute(path, ec);
            if (ec) canonical = path.lexically_normal();
        }
        return canonical.lexically_normal().generic_string();
    }

    static std::string stateCacheKey(const Aura::SDK::PluginDescriptor& descriptor) {
        return descriptor.isValid() ? descriptor.stateCacheKey() : std::string{};
    }

    void loadCache() {
        std::ifstream file(getCachePath(), std::ios::binary);
        if (!file.is_open()) return;

        uint32_t magic = 0;
        uint32_t version = 0;
        uint32_t count = 0;
        if (!file.read(reinterpret_cast<char*>(&magic), sizeof(magic)) ||
            !file.read(reinterpret_cast<char*>(&version), sizeof(version)) ||
            magic != kCacheMagic || version != kCacheVersion ||
            !file.read(reinterpret_cast<char*>(&count), sizeof(count))) return;
        if (count > 10000) return; // INDUSTRIAL: Sanity limit

        std::map<std::string, PluginMetadata> loaded;
        auto readString = [&file](std::string& value) {
            constexpr uint32_t kMaxFieldBytes = 1024 * 1024;
            uint32_t length = 0;
            if (!file.read(reinterpret_cast<char*>(&length), sizeof(length)) || length > kMaxFieldBytes) {
                return false;
            }
            value.resize(length);
            return length == 0 || static_cast<bool>(file.read(value.data(), length));
        };

        for(uint32_t i=0; i<count; ++i) {
            PluginMetadata meta;
            if (!readString(meta.name) || !readString(meta.path) ||
                !file.read(reinterpret_cast<char*>(&meta.type), sizeof(meta.type)) ||
                !file.read(reinterpret_cast<char*>(&meta.subtype), sizeof(meta.subtype))) {
                return;
            }
            uint64_t savedFingerprint = 0;
            if (!file.read(reinterpret_cast<char*>(&savedFingerprint), sizeof(savedFingerprint))) return;
            const auto format = PluginAdmission::formatForPath(meta.path);
            // Cache files are not an admission bypass. Re-apply the same
            // path/symlink/format gate used by filesystem scanning before a
            // persisted record becomes usable.
            if (format.empty() || !PluginAdmission::isSafeCandidate(meta.path, format)) {
                continue;
            }
            const auto currentFingerprint = fingerprintForPath(meta.path);
            if (!currentFingerprint || *currentFingerprint != savedFingerprint) {
                continue;
            }
            meta.fingerprint = savedFingerprint;
            loaded[cacheKey(meta.path)] = std::move(meta);
        }

        std::lock_guard<std::mutex> lock(m_mutex);
        m_cache = std::move(loaded);
        m_cacheGeneration.fetch_add(1, std::memory_order_acq_rel);
        loadBlacklist();
    }

    bool isBlacklisted(const std::string& pluginPath) const {
        const std::string key = cacheKey(pluginPath);
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_blacklist.find(key);
        if (it == m_blacklist.end()) return false;
        const auto fingerprintIt = m_blacklistFingerprints.find(key);
        // Legacy entries without a fingerprint remain conservatively
        // blacklisted until explicitly cleared.
        if (fingerprintIt == m_blacklistFingerprints.end()) return true;
        const auto current = fingerprintForPath(key);
        return current && *current == fingerprintIt->second;
    }

    uint32_t blacklistReason(const std::string& pluginPath) const {
        const std::string key = cacheKey(pluginPath);
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_blacklist.find(key);
        return it == m_blacklist.end() ? 0u : it->second;
    }

    std::vector<std::pair<std::string, uint32_t>> blacklistSnapshot() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<std::pair<std::string, uint32_t>> result;
        result.reserve(m_blacklist.size());
        for (const auto& entry : m_blacklist) result.push_back(entry);
        return result;
    }

    bool shouldScan(const std::string& pluginPath) const {
        return !pluginPath.empty() && !isBlacklisted(pluginPath);
    }

    // Cache entries are usable only while the plugin artifact is unchanged.
    // The signature deliberately covers bundle contents, not just the bundle
    // directory timestamp, because many plugin installers preserve it.
    static std::optional<uint64_t> fingerprintForPath(const std::filesystem::path& path) {
        // Fingerprints are persisted and compared through cacheKey(). Use the
        // same canonical root while hashing, otherwise an equivalent relative
        // path and absolute path produce different identities because their
        // entry names differ inside the hash.
        const std::string canonical = cacheKey(path.generic_string());
        const std::filesystem::path root = canonical.empty() ? path :
                                            std::filesystem::path(canonical);
        std::error_code ec;
        if (std::filesystem::is_symlink(std::filesystem::symlink_status(root, ec)) || ec) {
            return std::nullopt;
        }
        if (!std::filesystem::exists(root, ec) || ec) return std::nullopt;
        uint64_t hash = 1469598103934665603ULL;
        const auto mix = [&hash](uint64_t value) {
            for (unsigned i = 0; i < sizeof(value); ++i) {
                hash ^= static_cast<uint8_t>(value >> (i * 8));
                hash *= 1099511628211ULL;
            }
        };
        const auto mixString = [&mix](const std::string& value) {
            for (const unsigned char byte : value) {
                mix(static_cast<uint64_t>(byte));
            }
            // Keep concatenated path names unambiguous and independent of
            // std::hash implementation details across libc++/libstdc++.
            mix(0xffu);
        };
        const auto addEntry = [&](const std::filesystem::path& entry) {
            std::error_code entryEc;
            const auto status = std::filesystem::symlink_status(entry, entryEc);
            if (entryEc || std::filesystem::is_symlink(status)) return false;
            mixString(entry.lexically_normal().generic_string());
            mix(static_cast<uint64_t>(status.type()));
            if (std::filesystem::is_regular_file(status)) {
                const auto size = std::filesystem::file_size(entry, entryEc);
                if (entryEc) return false;
                mix(static_cast<uint64_t>(size));
                // Metadata alone can miss an in-place binary update when the
                // file keeps the same size and the filesystem timestamp has
                // coarse resolution. Include the actual bytes in the cache
                // identity so stale plugin admission data cannot survive an
                // update.
                std::ifstream content(entry, std::ios::binary);
                if (!content.is_open()) return false;
                std::array<char, 64 * 1024> chunk{};
                while (content) {
                    content.read(chunk.data(), static_cast<std::streamsize>(chunk.size()));
                    const auto count = content.gcount();
                    for (std::streamsize index = 0; index < count; ++index)
                        mix(static_cast<uint8_t>(chunk[static_cast<size_t>(index)]));
                }
                if (!content.eof()) return false;
            } else if (std::filesystem::is_directory(status)) {
                // Directory metadata and file mtimes are intentionally not
                // part of identity: equivalent bundle copies can have
                // different creation order/timestamps. Sorted paths, file
                // sizes, and file bytes are the stable content identity.
                mix(0xd1u);
            }
            return true;
        };

        if (std::filesystem::is_regular_file(root, ec)) {
            return addEntry(root) ? std::optional<uint64_t>(hash) : std::nullopt;
        }
        if (!std::filesystem::is_directory(root, ec)) return std::nullopt;
        if (!addEntry(root)) return std::nullopt;
        std::vector<std::filesystem::path> entries;
        std::filesystem::recursive_directory_iterator it(
            root, std::filesystem::directory_options::skip_permission_denied, ec);
        const std::filesystem::recursive_directory_iterator end;
        for (; it != end; it.increment(ec)) {
            if (ec) return std::nullopt;
            entries.push_back(it->path());
        }
        std::sort(entries.begin(), entries.end(), [](const auto& left, const auto& right) {
            return left.lexically_normal().generic_string() < right.lexically_normal().generic_string();
        });
        for (const auto& entry : entries) {
            if (!addEntry(entry)) return std::nullopt;
        }
        return hash;
    }

    bool isCacheFresh(const std::string& pluginPath) const {
        const std::string key = cacheKey(pluginPath);
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_cache.find(key);
        if (it == m_cache.end()) return false;
        const auto fingerprint = fingerprintForPath(pluginPath);
        if (!fingerprint) return false;
        return it->second.fingerprint != 0 && *fingerprint == it->second.fingerprint;
    }

    void recordScanFailure(const std::string& pluginPath, uint32_t reasonCode = 1) {
        if (pluginPath.empty()) return;
        const std::string key = cacheKey(pluginPath);
        const auto fingerprint = fingerprintForPath(pluginPath);
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            m_blacklist[key] = reasonCode;
            if (fingerprint) m_blacklistFingerprints[key] = *fingerprint;
            else m_blacklistFingerprints.erase(key);
        }
        saveBlacklist();
    }

    void clearScanFailure(const std::string& pluginPath) {
        const std::string key = cacheKey(pluginPath);
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            m_blacklist.erase(key);
            m_blacklistFingerprints.erase(key);
        }
        saveBlacklist();
    }

    void recordScanSuccess(const std::string& pluginPath) {
        if (pluginPath.empty()) return;
        const std::string key = cacheKey(pluginPath);
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            m_blacklist.erase(key);
            m_blacklistFingerprints.erase(key);
        }
        saveBlacklist();
    }

    void startBackgroundScan() {
        std::lock_guard<std::mutex> scanLock(m_scanMutex);
        if (m_scanThread.joinable()) {
            m_stopScan.store(true, std::memory_order_release);
            m_scanThread.join();
        }
        m_stopScan.store(false, std::memory_order_release);
        m_scanThread = std::thread([this]() {
#ifdef __APPLE__
            AudioComponentDescription desc{};
            desc.componentType = kAudioUnitType_Effect;
            AudioComponent comp = nullptr;
            while ((comp = AudioComponentFindNext(comp, &desc))) {
                if (m_stopScan.load(std::memory_order_acquire)) break;
                CFStringRef name = nullptr;
                AudioComponentCopyName(comp, &name);
                if (!name) continue;

                PluginMetadata meta;
                char buf[256];
                const bool converted = CFStringGetCString(name, buf, sizeof(buf), kCFStringEncodingUTF8);
                if (!converted) {
                    CFRelease(name);
                    continue;
                }
                meta.name = buf;
                meta.type = desc.componentType;
                meta.subtype = desc.componentSubType;
                CFRelease(name);

                // AudioComponent enumeration does not provide a stable
                // bundle path. Do not publish a path-less cache entry: it
                // cannot be fingerprinted or invalidated safely. Filesystem
                // scanning remains the authoritative admission source.
                if (meta.path.empty()) continue;

                {
                    std::lock_guard<std::mutex> lock(m_mutex);
                    // Cache identity is the canonical plugin path, not the
                    // display name. Two vendors commonly ship components
                    // with the same name; name-keying silently overwrites
                    // one admission record and breaks fingerprint checks.
                    m_cache[cacheKey(meta.path)] = std::move(meta);
                    m_cacheGeneration.fetch_add(1, std::memory_order_release);
                }
            }
#endif
            if (!m_stopScan.load(std::memory_order_acquire)) (void)saveCache();
        });
    }

    void stopBackgroundScan() {
        std::lock_guard<std::mutex> scanLock(m_scanMutex);
        m_stopScan.store(true, std::memory_order_release);
        if (m_scanThread.joinable()) m_scanThread.join();
    }

    bool saveCache() {
        std::string path = getCachePath().string();
        static std::atomic<uint64_t> shadowSequence{0};
        const std::string shadowPath = path + ".shadow-" +
            std::to_string(static_cast<unsigned long long>(::getpid())) + "-" +
            std::to_string(shadowSequence.fetch_add(1, std::memory_order_relaxed));

        const uint64_t snapshotGeneration = m_cacheGeneration.load(std::memory_order_acquire);
        std::vector<uint8_t> buffer;
        buffer.reserve(1024 * 64);
        
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            uint32_t count = 0;
            for (const auto& [key, meta] : m_cache) {
                (void)key;
                if (!meta.path.empty() && fingerprintForPath(meta.path)) ++count;
            }
            const uint32_t magic = kCacheMagic;
            const uint32_t version = kCacheVersion;
            buffer.insert(buffer.end(), reinterpret_cast<const uint8_t*>(&magic),
                          reinterpret_cast<const uint8_t*>(&magic) + 4);
            buffer.insert(buffer.end(), reinterpret_cast<const uint8_t*>(&version),
                          reinterpret_cast<const uint8_t*>(&version) + 4);
            buffer.insert(buffer.end(), (uint8_t*)&count, (uint8_t*)&count + 4);
            for (const auto& [key, meta] : m_cache) {
                if (meta.path.empty() || !fingerprintForPath(meta.path)) continue;
                uint32_t nLen = (uint32_t)meta.name.length();
                buffer.insert(buffer.end(), (uint8_t*)&nLen, (uint8_t*)&nLen + 4);
                buffer.insert(buffer.end(), (uint8_t*)meta.name.data(), (uint8_t*)meta.name.data() + nLen);
                uint32_t pLen = (uint32_t)meta.path.length();
                buffer.insert(buffer.end(), (uint8_t*)&pLen, (uint8_t*)&pLen + 4);
                buffer.insert(buffer.end(), (uint8_t*)meta.path.data(), (uint8_t*)meta.path.data() + pLen);
                buffer.insert(buffer.end(), (uint8_t*)&meta.type, (uint8_t*)&meta.type + 4);
                buffer.insert(buffer.end(), (uint8_t*)&meta.subtype, (uint8_t*)&meta.subtype + 4);
                const auto fingerprint = fingerprintForPath(meta.path);
                const uint64_t fingerprintValue = fingerprint.value_or(meta.fingerprint);
                buffer.insert(buffer.end(), reinterpret_cast<const uint8_t*>(&fingerprintValue),
                              reinterpret_cast<const uint8_t*>(&fingerprintValue) + 8);
            }
        }

        int fd = ::open(shadowPath.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) return false;
        size_t written = 0;
        while (written < buffer.size()) {
            const ssize_t count = ::write(fd, buffer.data() + written, buffer.size() - written);
            if (count <= 0) {
                ::close(fd);
                std::error_code cleanup;
                std::filesystem::remove(shadowPath, cleanup);
                return false;
            }
            written += static_cast<size_t>(count);
        }

#if defined(__APPLE__)
        ::fcntl(fd, F_FULLFSYNC);
#else
        ::fdatasync(fd);
#endif
        ::close(fd);
        // A scan may have published a newer cache while this snapshot was
        // being serialized. Never let the older snapshot win the final
        // rename; the next scan/save cycle will publish the newer generation.
        if (m_cacheGeneration.load(std::memory_order_acquire) != snapshotGeneration) {
            std::error_code cleanup;
            std::filesystem::remove(shadowPath, cleanup);
            return false;
        }
        if (::rename(shadowPath.c_str(), path.c_str()) == 0) {
            const auto parent = std::filesystem::path(path).parent_path();
            const int dirFd = ::open(parent.empty() ? "." : parent.c_str(), O_RDONLY | O_DIRECTORY);
            if (dirFd < 0) return false;
            const bool synced = ::fsync(dirFd) == 0;
            ::close(dirFd);
            return synced;
        } else {
            std::error_code cleanup;
            std::filesystem::remove(shadowPath, cleanup);
            return false;
        }
        return false;
    }

    void loadBlacklist() {
        std::ifstream file(getCachePath().string() + ".blacklist");
        if (!file.is_open()) return;
        std::map<std::string, uint32_t> loaded;
        std::string line;
        std::map<std::string, uint64_t> loadedFingerprints;
        while (std::getline(file, line)) {
            if (line.empty()) continue;
            std::istringstream record(line);
            std::string path;
            uint32_t reason = 0;
            uint64_t fingerprint = 0;
            if (!(record >> std::quoted(path) >> reason)) continue;
            if (path.size() <= 4096) {
                const std::string key = cacheKey(path);
                loaded[key] = reason;
                if (record >> fingerprint) loadedFingerprints[key] = fingerprint;
            }
        }
        m_blacklist = std::move(loaded);
        m_blacklistFingerprints = std::move(loadedFingerprints);
    }

    bool saveBlacklist() const {
        const std::string path = getCachePath().string() + ".blacklist";
        static std::atomic<uint64_t> shadowSequence{0};
        const std::string shadow = path + ".shadow-" +
            std::to_string(static_cast<unsigned long long>(::getpid())) + "-" +
            std::to_string(shadowSequence.fetch_add(1, std::memory_order_relaxed));
        std::ofstream file(shadow, std::ios::trunc);
        if (!file.is_open()) return false;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            for (const auto& [plugin, reason] : m_blacklist) {
                const auto fingerprint = m_blacklistFingerprints.find(plugin);
                file << std::quoted(plugin) << ' ' << reason;
                if (fingerprint != m_blacklistFingerprints.end())
                    file << ' ' << fingerprint->second;
                file << '\n';
            }
        }
        file.close();
        std::error_code ec;
        std::filesystem::rename(shadow, path, ec);
        if (ec) {
            std::filesystem::remove(shadow, ec);
            return false;
        }
        const auto parent = std::filesystem::path(path).parent_path();
        const int dirFd = ::open(parent.empty() ? "." : parent.c_str(), O_RDONLY | O_DIRECTORY);
        if (dirFd < 0) return false;
        const bool synced = ::fsync(dirFd) == 0;
        ::close(dirFd);
        return synced;
    }

    std::map<std::string, PluginMetadata> getCache() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_cache;
    }

private:
    PluginCacheManager() { loadCache(); }
    ~PluginCacheManager() { stopBackgroundScan(); }
    PluginCacheManager(const PluginCacheManager&) = delete;
    PluginCacheManager& operator=(const PluginCacheManager&) = delete;
    std::map<std::string, PluginMetadata> m_cache;
    mutable std::mutex m_mutex;
    std::atomic<uint64_t> m_cacheGeneration{0};
    std::mutex m_scanMutex;
    std::thread m_scanThread;
    std::atomic<bool> m_stopScan{false};
    std::map<std::string, uint32_t> m_blacklist;
    std::map<std::string, uint64_t> m_blacklistFingerprints;
};

} // namespace Aura::Core::Plugins
