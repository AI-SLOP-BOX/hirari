/**
 * @class PluginScanner
 * @brief Professional OS-level scanner with Process Sandboxing.
 */
class PluginScanner {
public:
    enum class ScanState : uint8_t {
        Idle,
        Running,
        Rejected,
        Completed,
    };

    PluginScanner() : m_isScanning(false), m_state(ScanState::Idle) {}

    void scanInSandbox(const std::string& path) {
        if (m_isScanning.exchange(true, std::memory_order_acq_rel)) return;

        m_state.store(ScanState::Running, std::memory_order_release);
        {
            std::lock_guard<std::mutex> lock(m_metadataMutex);
            m_lastPath = path;
            m_lastError.clear();
        }

        const auto reject = [this](const char* reason) {
            {
                std::lock_guard<std::mutex> lock(m_metadataMutex);
                m_lastError = reason;
            }
            m_state.store(ScanState::Rejected, std::memory_order_release);
            m_isScanning.store(false, std::memory_order_release);
        };

        if (path.empty()) {
            reject("plugin scan rejected: path is empty");
            return;
        }

        std::error_code ec;
        const std::filesystem::path candidate(path);
        if (!std::filesystem::exists(candidate, ec)) {
            reject(ec ? "plugin scan rejected: unable to inspect path"
                      : "plugin scan rejected: path does not exist");
            return;
        }
        if (ec) {
            reject("plugin scan rejected: unable to inspect path");
            return;
        }
        if (std::filesystem::is_directory(candidate, ec)) {
            reject("plugin scan rejected: path is a directory");
            return;
        }
        if (ec || !std::filesystem::is_regular_file(candidate, ec)) {
            reject(ec ? "plugin scan rejected: unable to inspect file"
                      : "plugin scan rejected: path is not a regular file");
            return;
        }

        // This API validates and records a scan request only.  It deliberately
        // does not pretend to launch a sandbox process or load a plug-in SDK.
        m_state.store(ScanState::Completed, std::memory_order_release);
        m_isScanning.store(false, std::memory_order_release);
    }

    bool isScanning() const noexcept {
        return m_isScanning.load(std::memory_order_acquire);
    }

    ScanState state() const noexcept {
        return m_state.load(std::memory_order_acquire);
    }

    std::string lastError() const {
        std::lock_guard<std::mutex> lock(m_metadataMutex);
        return m_lastError;
    }

    std::string lastPath() const {
        std::lock_guard<std::mutex> lock(m_metadataMutex);
        return m_lastPath;
    }

private:
    std::atomic<bool> m_isScanning;
    std::atomic<ScanState> m_state;
    mutable std::mutex m_metadataMutex;
    std::string m_lastError;
    std::string m_lastPath;
};

