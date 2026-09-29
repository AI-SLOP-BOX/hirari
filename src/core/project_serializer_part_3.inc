    static bool saveAtomic(const std::string& path, const ProjectState& state) {
        if (path.empty()) return false;
        auto buffer = serialize(state);
        if (buffer.size() < sizeof(uint32_t) * 2u) return false;
        return hirari_save_native_project_bytes(path.data(), path.size(),
                                                buffer.data(), buffer.size()) != 0;
    }
