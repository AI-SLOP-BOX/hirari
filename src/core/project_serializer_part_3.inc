    static std::vector<uint8_t> serialize(const ProjectState& state) {
        std::stringstream ss(std::ios::binary | std::ios::out);
        writeToStream(ss, state);
        std::string str = ss.str();
        std::vector<uint8_t> bytes(str.begin(), str.end());
        // Version 27+ uses a real CRC32 trailer over the complete payload;
        // version 28 additionally preserves region identities.
        // This replaces the old integrity-by-obfuscation assumption and lets
        // readers reject torn/corrupted native snapshots deterministically.
        if (state.version >= 27) {
            const uint32_t checksum = calculateCRC32(bytes.data(), bytes.size());
            const auto* raw = reinterpret_cast<const uint8_t*>(&checksum);
            bytes.insert(bytes.end(), raw, raw + sizeof(checksum));
        }
        return bytes;
    }

    static bool save(const std::string& path, const ProjectState& state) {
        std::ofstream file(path, std::ios::binary);
        if (!file.is_open()) return false;
        auto buffer = serialize(state);
        file.write(reinterpret_cast<const char*>(buffer.data()), buffer.size());
        file.flush();
        return file.good();
    }

    static bool saveAtomic(const std::string& path, const ProjectState& state) {
        if (path.empty()) return false;
        static std::atomic<uint64_t> tempSequence{0};
        const std::string temporary = path + ".tmp-" + std::to_string(static_cast<unsigned long long>(::getpid())) +
            "-" + std::to_string(static_cast<unsigned long long>(tempSequence.fetch_add(1, std::memory_order_relaxed)));
        std::error_code ec;
        if (!save(temporary, state)) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
        // Flush the file before publishing the rename. A successful rename
        // alone does not protect a project from a power loss while the temp
        // file is still only in the kernel page cache.
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::filesystem::remove(temporary, ec);
            return false;
        }
        ::close(fd);
        const auto expectedSize = std::filesystem::file_size(temporary, ec);
        if (ec || expectedSize < sizeof(uint32_t) * 2u) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
        std::filesystem::rename(temporary, path, ec);
        if (ec) std::filesystem::remove(temporary, ec);
        if (!ec) {
            const auto parent = std::filesystem::path(path).parent_path();
            const std::string parentPath = parent.empty() ? "." : parent.string();
            const int dirFd = ::open(parentPath.c_str(), O_RDONLY | O_DIRECTORY);
            if (dirFd < 0) {
                ec = std::make_error_code(std::errc::io_error);
            } else {
                if (::fsync(dirFd) != 0) ec = std::make_error_code(std::errc::io_error);
                ::close(dirFd);
            }
        }
        return !ec;
    }

