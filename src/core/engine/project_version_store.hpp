#pragma once

#include <algorithm>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>
#include <mutex>

namespace Aura::Core::Engine {

// Small, dependency-free project history store used by the UI and headless CLI.
// Each revision is immutable and written atomically, allowing recovery after a
// failed save without coupling history to the live project file.
class ProjectVersionStore {
public:
    struct Revision { uint64_t id = 0; std::string file; uint64_t bytes = 0; };
    struct DiffSummary { uint64_t changed = 0; uint64_t added = 0; uint64_t removed = 0; std::vector<std::pair<uint64_t,uint64_t>> ranges; };

    explicit ProjectVersionStore(std::filesystem::path directory)
        : m_directory(std::move(directory)) {}

    bool append(const std::vector<uint8_t>& data, Revision* out = nullptr) {
        if (data.empty()) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        std::error_code ec;
        std::filesystem::create_directories(m_directory, ec);
        if (ec) return false;
        const auto revisions = list_unlocked();
        const uint64_t id = revisions.empty() ? 1 : revisions.back().id + 1;
        const auto target = m_directory / ("revision-" + std::to_string(id) + ".aura");
        const auto temp = target.string() + ".tmp-" + std::to_string(m_tempSequence++);
        { std::ofstream f(temp, std::ios::binary | std::ios::trunc); if (!f) return false;
          f.write(reinterpret_cast<const char*>(data.data()), static_cast<std::streamsize>(data.size()));
          f.flush(); if (!f) return false; }
        std::filesystem::rename(temp, target, ec);
        if (ec) { std::filesystem::remove(temp, ec); return false; }
        if (out) *out = Revision{id, target.filename().string(), data.size()};
        return true;
    }

    std::vector<Revision> list() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return list_unlocked();
    }

private:
    std::vector<Revision> list_unlocked() const {
        std::vector<Revision> result; std::error_code ec;
        if (!std::filesystem::is_directory(m_directory, ec)) return result;
        for (const auto& e : std::filesystem::directory_iterator(m_directory, ec)) {
            if (ec || !e.is_regular_file()) continue;
            const auto name = e.path().filename().string();
            if (name.rfind("revision-", 0) != 0 || e.path().extension() != ".aura") continue;
            try { const auto id = std::stoull(name.substr(9, name.size() - 14));
                  result.push_back({id, name, e.file_size()}); } catch (...) {}
        }
        std::sort(result.begin(), result.end(), [](const Revision& a, const Revision& b){ return a.id < b.id; });
        return result;
    }

public:

    bool load(uint64_t id, std::vector<uint8_t>& data) const {
        const auto path = m_directory / ("revision-" + std::to_string(id) + ".aura");
        std::ifstream f(path, std::ios::binary); if (!f) return false;
        data.assign(std::istreambuf_iterator<char>(f), {}); return !data.empty();
    }
    std::vector<uint64_t> diff(uint64_t older, uint64_t newer) const {
        std::vector<uint8_t> a,b; if(!load(older,a)||!load(newer,b)) return {};
        const size_t n=std::max(a.size(),b.size()); std::vector<uint64_t> out;
        for(size_t i=0;i<n;++i){const uint8_t av=i<a.size()?a[i]:0,bv=i<b.size()?b[i]:0;if(av!=bv)out.push_back(static_cast<uint64_t>(i));}
        return out;
    }
    DiffSummary summarizeDiff(uint64_t older, uint64_t newer) const {
        std::vector<uint8_t> a, b; DiffSummary summary; if (!load(older, a) || !load(newer, b)) return summary;
        const size_t common = std::min(a.size(), b.size()); size_t rangeStart = SIZE_MAX;
        auto close = [&](size_t end) { if (rangeStart != SIZE_MAX) { summary.ranges.emplace_back(rangeStart, end); rangeStart = SIZE_MAX; } };
        for (size_t i = 0; i < common; ++i) { if (a[i] != b[i]) { ++summary.changed; if (rangeStart == SIZE_MAX) rangeStart = i; } else close(i); }
        close(common); if (b.size() > a.size()) summary.added = b.size() - a.size(); else summary.removed = a.size() - b.size(); return summary;
    }

private:
    std::filesystem::path m_directory;
    mutable std::mutex m_mutex;
    uint64_t m_tempSequence = 0;
};
}
