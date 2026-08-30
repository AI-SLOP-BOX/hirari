#pragma once

#include <algorithm>
#include <cctype>
#include <filesystem>
#include <fstream>
#include <string>
#include <unordered_set>
#include <vector>

namespace Aura::Core::IO {

struct MediaCatalogEntry {
    std::filesystem::path path;
    uintmax_t bytes = 0;
    std::string extension;
    std::string tag;
    bool favorite = false;
    bool used = false;
};

class MediaCatalog {
public:
    bool scan(const std::filesystem::path& root,
              const std::vector<std::filesystem::path>& usedFiles = {}) {
        m_entries.clear(); m_used.clear();
        std::error_code ec;
        for (const auto& p : usedFiles) m_used.insert(normalize(p));
        if (!std::filesystem::is_directory(root, ec)) return false;
        for (std::filesystem::recursive_directory_iterator it(root, ec), end; it != end && !ec; it.increment(ec)) {
            if (!it->is_regular_file(ec)) continue;
            const auto ext = lower(it->path().extension().string());
            if (!isAudio(ext)) continue;
            MediaCatalogEntry e; e.path = it->path(); e.extension = ext;
            e.bytes = it->file_size(ec); e.used = m_used.count(normalize(e.path)) != 0;
            m_entries.push_back(std::move(e));
        }
        std::sort(m_entries.begin(), m_entries.end(), [](const auto& a, const auto& b){ return a.path < b.path; });
        return !ec;
    }

    void setTag(const std::filesystem::path& path, std::string tag) {
        for (auto& e : m_entries) if (normalize(e.path) == normalize(path)) e.tag = std::move(tag);
    }
    void setFavorite(const std::filesystem::path& path, bool value) {
        for (auto& e : m_entries) if (normalize(e.path) == normalize(path)) e.favorite = value;
    }
    std::vector<MediaCatalogEntry> search(const std::string& query, bool favoritesOnly = false) const {
        const auto q = lower(query); std::vector<MediaCatalogEntry> out;
        for (const auto& e : m_entries) {
            const auto name = lower(e.path.filename().string());
            if ((!favoritesOnly || e.favorite) && (q.empty() || name.find(q) != std::string::npos || lower(e.tag).find(q) != std::string::npos)) out.push_back(e);
        }
        return out;
    }
    std::vector<MediaCatalogEntry> unused() const { std::vector<MediaCatalogEntry> out; for (const auto& e : m_entries) if (!e.used) out.push_back(e); return out; }
    std::vector<std::vector<MediaCatalogEntry>> duplicates() const {
        std::vector<std::vector<MediaCatalogEntry>> groups;
        for (size_t i=0;i<m_entries.size();++i) { std::vector<MediaCatalogEntry> g{m_entries[i]}; for(size_t j=i+1;j<m_entries.size();++j) if(m_entries[j].bytes==m_entries[i].bytes&&m_entries[j].extension==m_entries[i].extension) g.push_back(m_entries[j]); if(g.size()>1) groups.push_back(std::move(g)); }
        return groups;
    }
    const std::vector<MediaCatalogEntry>& entries() const noexcept { return m_entries; }

private:
    static std::string lower(std::string s) { for (auto& c : s) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c))); return s; }
    static std::string normalize(const std::filesystem::path& p) { std::error_code ec; auto a = std::filesystem::weakly_canonical(p, ec); return (ec ? p : a).string(); }
    static bool isAudio(const std::string& ext) { static const std::unordered_set<std::string> k{ ".wav", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".m4a", ".caf" }; return k.count(ext) != 0; }
    std::vector<MediaCatalogEntry> m_entries; std::unordered_set<std::string> m_used;
};
}
