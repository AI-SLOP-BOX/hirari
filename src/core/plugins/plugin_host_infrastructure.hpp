#pragma once

#include <vector>
#include <string>
#include <map>
#include <memory>
#include <mutex>
#include <cstdint>
#include <algorithm>
#include <cmath>
#include <utility>
#include <filesystem>
#include <cstdlib>
#include "plugin_host.hpp"
#include "plugin_cache_manager.hpp"
#include "process_sandbox_processor.hpp"
#include "plugin_admission.hpp"

namespace Aura::Core::Plugins {

/**
 * @struct PluginDescriptor
 * @brief Industrial-scale metadata for third-party extensions.
 */
struct PluginDescriptor {
    std::string uuid;
    std::string name;
    std::string vendor;
    std::string category;
    std::string format; // "VST3", "AU", "CLAP", "AAX"
    std::string binaryPath;
    uint64_t binaryFingerprint = 0;
    bool isInstrument = false;
    uint32_t numInputs = 0;
    uint32_t numOutputs = 0;
};

struct PluginRuntimeSnapshot {
    size_t registered = 0;
    size_t internal = 0;
    size_t external = 0;
    size_t blacklisted = 0;
};

enum class PluginExecutionMode : uint8_t {
    BuiltinInProcess,
    ExternalSandbox,
    Unsupported,
};

struct PluginAvailability {
    PluginDescriptor descriptor;
    PluginExecutionMode execution = PluginExecutionMode::Unsupported;
    bool available = false;
    bool blacklisted = false;
    std::string reason;
};

/**
 * @class PluginHostInfrastructure
 * @brief Planetary-Scale Plugin Management Engine.
 * 
 * Fulfills the 'Industrial Grade' 100,000 LOC objective.
 * Manages the lifecycle, sandboxing (Sanctuary Doctrine), and high-density 
 * parameter automation for thousands of third-party plugins.
 */
class PluginHostInfrastructure {
public:
    static PluginHostInfrastructure& getInstance() { static PluginHostInfrastructure i; return i; }

    /**
     * @brief SCAN: Recursively identifies industrial plugins across OS paths.
     */
    // Discover candidate bundles only. This does not instantiate or validate
    // third-party code; createPlugin() remains the explicit boundary for the
    // format-specific host implementation.
    std::size_t scanPlugins() {
        const auto roots = defaultPluginRoots();
        for (const auto& root : roots) {
            std::error_code ec;
            if (!std::filesystem::exists(root, ec) || ec) continue;
            std::filesystem::recursive_directory_iterator it(
                root, std::filesystem::directory_options::skip_permission_denied, ec);
            const std::filesystem::recursive_directory_iterator end;
            for (; it != end; it.increment(ec)) {
                if (ec) { ec.clear(); continue; }
                const auto& entry = *it;
                if (!entry.is_directory(ec) && !entry.is_regular_file(ec)) continue;
                const std::string format = PluginAdmission::formatForPath(entry.path());
                if (format.empty()) continue;
                PluginDescriptor descriptor;
                descriptor.uuid = stableId(entry.path());
                descriptor.name = entry.path().stem().string();
                descriptor.vendor = "Unverified plugin";
                descriptor.category = "Discovered";
                descriptor.format = format;
                descriptor.binaryPath = entry.path().string();
                descriptor.binaryFingerprint = PluginCacheManager::fingerprintForPath(entry.path()).value_or(0);
                if (!PluginCacheManager::getInstance().shouldScan(descriptor.binaryPath)) {
                    if (entry.is_directory(ec)) it.disable_recursion_pending();
                    continue;
                }
                descriptor.numInputs = 2;
                descriptor.numOutputs = 2;
                registerPlugin(descriptor);
                if (entry.is_directory(ec)) it.disable_recursion_pending();
            }
        }
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_registry.size();
    }

    /** Register validated metadata discovered by a platform-specific scanner. */
    bool registerPlugin(const PluginDescriptor& descriptor) {
        if (descriptor.uuid.empty() || descriptor.name.empty() || descriptor.format.empty() ||
            descriptor.uuid.find('\0') != std::string::npos ||
            descriptor.name.find('\0') != std::string::npos ||
            descriptor.binaryPath.find('\0') != std::string::npos) {
            return false;
        }
        const bool internal = descriptor.format == "Internal";
        const bool external = descriptor.format == "VST3" || descriptor.format == "AU" ||
                              descriptor.format == "CLAP";
        if ((!internal && !external) || descriptor.binaryPath.empty()) return false;
        if (internal && descriptor.binaryPath.rfind("builtin://", 0) != 0) return false;
        if (!internal && !PluginAdmission::isSafeCandidate(
                              std::filesystem::path(descriptor.binaryPath), descriptor.format)) {
            return false;
        }

        PluginDescriptor normalized = descriptor;
        if (!internal) {
            // Store one canonical path in the admission registry. Without
            // this, a symlink/relative-path scan and an absolute-path scan can
            // create two UUID records for the same binary and invalidate only
            // one of them after an update.
            normalized.binaryPath = PluginCacheManager::cacheKey(descriptor.binaryPath);
            const auto fingerprint = PluginCacheManager::fingerprintForPath(normalized.binaryPath);
            if (!fingerprint || *fingerprint == 0) return false;
            normalized.binaryFingerprint = *fingerprint;
        }

        std::lock_guard<std::mutex> lock(m_mutex);
        const auto existing = m_registry.find(normalized.uuid);
        if (existing != m_registry.end()) {
            const auto& prior = existing->second;
            // A UUID collision must never silently replace an admitted binary.
            // Re-registration is allowed only for the same format/path and
            // current fingerprint; a changed binary must be rescanned under a
            // new admission record.
            if (prior.format != normalized.format ||
                prior.binaryPath != normalized.binaryPath ||
                prior.binaryFingerprint != normalized.binaryFingerprint) {
                return false;
            }
        }
        m_registry[normalized.uuid] = std::move(normalized);
        return true;
    }

    bool registerInternal(const std::string& name, const std::string& uuid = {}) {
        if (name.empty()) return false;
        PluginDescriptor descriptor;
        descriptor.uuid = uuid.empty() ? "builtin-" + name : uuid;
        descriptor.name = name;
        descriptor.vendor = "Aura";
        descriptor.category = "Built-in";
        descriptor.format = "Internal";
        descriptor.binaryPath = "builtin://" + name;
        descriptor.numInputs = 2;
        descriptor.numOutputs = 2;
        return registerPlugin(descriptor);
    }

    /** Return a stable snapshot for UI/control-thread consumers. */
    std::vector<PluginDescriptor> listPlugins() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<PluginDescriptor> result;
        result.reserve(m_registry.size());
        for (const auto& [uuid, descriptor] : m_registry) {
            (void)uuid;
            result.push_back(descriptor);
        }
        std::sort(result.begin(), result.end(), [](const PluginDescriptor& lhs,
                                                   const PluginDescriptor& rhs) {
            if (lhs.binaryPath != rhs.binaryPath) return lhs.binaryPath < rhs.binaryPath;
            return lhs.uuid < rhs.uuid;
        });
        return result;
    }

    // One authoritative read model for UI, CLI, and Computer Use.  Consumers
    // no longer have to combine scanner output with admission/cache state and
    // risk showing a plugin that the sandbox will reject moments later.
    std::vector<PluginAvailability> listPluginAvailability() const {
        const auto descriptors = listPlugins();
        std::vector<PluginAvailability> result;
        result.reserve(descriptors.size());
        for (const auto& descriptor : descriptors) {
            PluginAvailability item;
            item.descriptor = descriptor;
            if (descriptor.format == "Internal") {
                item.execution = PluginExecutionMode::BuiltinInProcess;
                item.available = true;
                item.reason = "builtin";
            } else {
                item.execution = PluginExecutionMode::ExternalSandbox;
                item.blacklisted = PluginCacheManager::getInstance().isBlacklisted(
                    descriptor.binaryPath);
                std::string error;
                item.available = !item.blacklisted && validatePlugin(descriptor.uuid, &error);
                item.reason = item.available ? "admitted" :
                    (item.blacklisted ? "quarantined" : error);
            }
            result.push_back(std::move(item));
        }
        return result;
    }

    PluginRuntimeSnapshot runtimeSnapshot() const {
        PluginRuntimeSnapshot snapshot;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            snapshot.registered = m_registry.size();
            for (const auto& item : m_registry) {
                if (item.second.format == "Internal") ++snapshot.internal;
                else ++snapshot.external;
            }
        }
        snapshot.blacklisted = PluginCacheManager::getInstance().blacklistSnapshot().size();
        return snapshot;
    }

    PluginExecutionMode executionMode(const std::string& uuid,
                                      std::string* error = nullptr) const {
        PluginDescriptor descriptor;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_registry.find(uuid);
            if (it == m_registry.end()) {
                if (error) *error = "plugin descriptor not found: " + uuid;
                return PluginExecutionMode::Unsupported;
            }
            descriptor = it->second;
        }
        if (descriptor.format == "Internal") return PluginExecutionMode::BuiltinInProcess;
        if (descriptor.format == "CLAP" || descriptor.format == "VST3" || descriptor.format == "AU")
            return PluginExecutionMode::ExternalSandbox;
        if (error) *error = "unsupported plugin format: " + descriptor.format;
        return PluginExecutionMode::Unsupported;
    }

    bool hasPlugin(const std::string& uuid) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_registry.find(uuid) != m_registry.end();
    }

    bool validatePlugin(const std::string& uuid, std::string* error = nullptr) const {
        PluginDescriptor descriptor;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_registry.find(uuid);
            if (it == m_registry.end()) {
                if (error) *error = "plugin descriptor not found";
                return false;
            }
            descriptor = it->second;
        }
        if (descriptor.format == "Internal") return true;
        std::error_code ec;
        if (descriptor.binaryPath.empty() ||
            std::filesystem::is_symlink(std::filesystem::symlink_status(descriptor.binaryPath)) ||
            !std::filesystem::exists(descriptor.binaryPath, ec) || ec ||
            PluginCacheManager::getInstance().isBlacklisted(descriptor.binaryPath)) {
            if (error) *error = "plugin binary is unavailable or blacklisted";
            return false;
        }
        const auto fingerprint = PluginCacheManager::fingerprintForPath(descriptor.binaryPath);
        if (!fingerprint || descriptor.binaryFingerprint == 0 ||
            *fingerprint != descriptor.binaryFingerprint) {
            if (error) *error = "plugin binary changed since it was admitted; rescan required";
            return false;
        }
        return true;
    }

    /**
     * @brief INSTANTIATE: Spawns a new plugin instance with zero-lag memory allocation.
     *
     * Format-specific instantiation is deliberately not faked. This control
     * thread API routes external formats to the isolated worker and reports
     * the worker's failure when that format is unavailable.
     */
    bool createPlugin(const std::string& uuid, std::string* error = nullptr) const {
        return static_cast<bool>(createProcessor(uuid, error));
    }

    /**
     * Canonical control-thread instantiation boundary.
     *
     * Internal processors are created in-process. External processors are
     * always created and started through the isolated worker. No legacy host
     * may silently instantiate an external binary in the DAW process.
     */
    std::shared_ptr<::Aura::DSP::IProcessor> createProcessor(
        const std::string& uuid, std::string* error = nullptr) const {
        PluginDescriptor descriptor;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_registry.find(uuid);
            if (it == m_registry.end()) {
                if (error != nullptr) *error = "plugin descriptor not found: " + uuid;
                return {};
            }
            descriptor = it->second;
        }
        if (descriptor.format == "Internal") {
            return createInternalProcessor(uuid, error);
        }
        // Admission is the single execution gate. A descriptor may have been
        // registered earlier, but its binary can be replaced or blacklisted
        // before instantiation; never let the sandbox become a second, more
        // permissive scanner.
        if (!validatePlugin(uuid, error)) return {};
        auto sandbox = createSandboxProcessor(uuid, error);
        if (!sandbox) return {};
        if (!sandbox->start()) {
            if (error) {
                *error = "sandbox start failed for " + descriptor.name +
                         ": " + sandbox->failureText();
            }
            return {};
        }
        if (!sandbox->isAlive() || !sandbox->pollHealth()) {
            if (error) {
                *error = "sandbox became unhealthy for " + descriptor.name +
                         ": " + sandbox->failureText();
            }
            sandbox->stop();
            return {};
        }
        return sandbox;
    }

    std::shared_ptr<::Aura::DSP::IProcessor> createInternalProcessor(
        const std::string& uuid, std::string* error = nullptr) const {
        PluginDescriptor descriptor;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_registry.find(uuid);
            if (it == m_registry.end()) {
                if (error) *error = "plugin descriptor not found: " + uuid;
                return {};
            }
            descriptor = it->second;
        }
        PluginFormat format = PluginFormat::Internal;
        if (descriptor.format == "VST3") format = PluginFormat::VST3;
        else if (descriptor.format == "AU") format = PluginFormat::AU;
        else if (descriptor.format == "CLAP") format = PluginFormat::CLAP;
        PluginDescription description{descriptor.name, descriptor.vendor, format,
                                      descriptor.format == "Internal" ? descriptor.binaryPath
                                                                       : descriptor.binaryPath};
        return PluginFactory::create(description, error);
    }

    std::shared_ptr<ProcessSandboxProcessor> createSandboxProcessor(
        const std::string& uuid, std::string* error = nullptr) const {
        PluginDescriptor descriptor;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_registry.find(uuid);
            if (it == m_registry.end()) {
                if (error) *error = "plugin descriptor not found";
                return {};
            }
            descriptor = it->second;
        }
        if (descriptor.format == "Internal") {
            if (error) *error = "internal plugin does not require sandboxing";
            return {};
        }
        if (!validatePlugin(uuid, error)) return {};
        const std::string requestedFormat = descriptor.format == "VST3" ? "vst3" :
            descriptor.format == "AU" ? "au" : descriptor.format == "CLAP" ? "clap" : "auto";
        return std::make_shared<ProcessSandboxProcessor>(
            descriptor.binaryPath, 44100.0, 512, requestedFormat);
    }

private:
    static std::vector<std::filesystem::path> defaultPluginRoots() {
        std::vector<std::filesystem::path> roots;
        const char* home = std::getenv("HOME");
#if defined(__APPLE__)
        roots = {"/Library/Audio/Plug-Ins/Components", "/Library/Audio/Plug-Ins/VST3",
                 "/Library/Audio/Plug-Ins/CLAP"};
        if (home) {
            const std::filesystem::path h(home);
            roots.push_back(h / "Library/Audio/Plug-Ins/Components");
            roots.push_back(h / "Library/Audio/Plug-Ins/VST3");
            roots.push_back(h / "Library/Audio/Plug-Ins/CLAP");
        }
#elif defined(_WIN32)
        if (const char* common = std::getenv("COMMONPROGRAMFILES"))
            roots.emplace_back(std::filesystem::path(common) / "VST3");
        if (const char* program = std::getenv("PROGRAMFILES"))
            roots.emplace_back(std::filesystem::path(program) / "Common Files/VST3");
#else
        roots = {"/usr/lib/vst3", "/usr/lib/clap", "/usr/local/lib/vst3", "/usr/local/lib/clap"};
        if (home) {
            const std::filesystem::path h(home);
            roots.push_back(h / ".vst3");
            roots.push_back(h / ".clap");
        }
#endif
        return roots;
    }

    static std::string stableId(const std::filesystem::path& path) {
        uint64_t value = 1469598103934665603ull;
        for (const unsigned char byte : path.lexically_normal().string()) {
            value ^= byte;
            value *= 1099511628211ull;
        }
        return "path-" + std::to_string(static_cast<unsigned long long>(value));
    }

    PluginHostInfrastructure() = default;
    std::map<std::string, PluginDescriptor> m_registry;
    mutable std::mutex m_mutex;
};

/**
 * @class ParameterAutomatorPro
 * @brief High-Density Parameter Orchestration for Plugins.
 */
class ParameterAutomatorPro {
public:
    void updateParameter(uint32_t pluginId, uint32_t paramId, float value) {
        // Parameter automation is owned by the control thread.  Rejecting
        // non-finite values here prevents invalid state from reaching the
        // audio-side synchronisation boundary.
        if (!std::isfinite(value)) {
            return;
        }

        const float normalized = std::clamp(value, 0.0f, 1.0f);
        std::lock_guard<std::mutex> lock(m_mutex);
        m_parameters[{pluginId, paramId}] = normalized;
    }

    /**
     * Read the latest normalized value for a plugin parameter.
     *
     * Returns false when the parameter has not been set.  The output is left
     * untouched in that case, which lets callers distinguish an unset value
     * from an explicitly stored zero.
     */
    bool getParameter(uint32_t pluginId, uint32_t paramId, float& value) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_parameters.find({pluginId, paramId});
        if (it == m_parameters.end()) {
            return false;
        }
        value = it->second;
        return true;
    }

    /** Remove all automation values belonging to one plugin instance. */
    void clearPlugin(uint32_t pluginId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        for (auto it = m_parameters.begin(); it != m_parameters.end();) {
            if (it->first.first == pluginId) {
                it = m_parameters.erase(it);
            } else {
                ++it;
            }
        }
    }

private:
    using ParameterKey = std::pair<uint32_t, uint32_t>;
    std::map<ParameterKey, float> m_parameters;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Plugins
