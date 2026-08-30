#pragma once

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <string>
#include <unordered_map>
#include <vector>

namespace Aura::Core::Plugins {

struct PluginCompatibilityRecord {
    std::string identifier;
    bool blacklisted = false;
    uint32_t scanFailures = 0;
    uint32_t crashCount = 0;
    std::string lastError;
};

class PluginCompatibilityRegistry {
public:
    void markScanFailure(const std::string& id, const std::string& error) {
        if (id.empty()) return; auto& r = m_records[id]; r.identifier = id; ++r.scanFailures; r.lastError = error;
    }
    void markCrash(const std::string& id) { if (id.empty()) return; auto& r=m_records[id]; r.identifier=id; ++r.crashCount; }
    void setBlacklisted(const std::string& id, bool value) { if (id.empty()) return; auto& r=m_records[id]; r.identifier=id; r.blacklisted=value; }
    bool isBlacklisted(const std::string& id) const { auto it=m_records.find(id); return it != m_records.end() && it->second.blacklisted; }
    const PluginCompatibilityRecord* find(const std::string& id) const { auto it=m_records.find(id); return it==m_records.end()?nullptr:&it->second; }
    std::vector<PluginCompatibilityRecord> records() const { std::vector<PluginCompatibilityRecord> out; for(const auto& [_,r]:m_records) out.push_back(r); std::sort(out.begin(),out.end(),[](const auto&a,const auto&b){return a.identifier<b.identifier;}); return out; }

    bool save(const std::filesystem::path& path) const {
        const auto temp = path.string()+".tmp"; std::ofstream f(temp, std::ios::trunc); if(!f) return false;
        f << "id\tblacklisted\tscan_failures\tcrashes\terror\n";
        for (const auto& r : records()) f << r.identifier << '\t' << (r.blacklisted?1:0) << '\t' << r.scanFailures << '\t' << r.crashCount << '\t' << r.lastError << '\n';
        f.flush(); if(!f) return false; std::error_code ec; std::filesystem::rename(temp,path,ec); if(ec) std::filesystem::remove(temp,ec); return !ec;
    }
    bool load(const std::filesystem::path& path) {
        std::ifstream f(path); if (!f) return false; std::string line; std::getline(f, line); if (line != "id\tblacklisted\tscan_failures\tcrashes\terror") return false;
        std::unordered_map<std::string, PluginCompatibilityRecord> next;
        while (std::getline(f, line)) { std::vector<std::string> fields; size_t p=0, n=0; while ((n=line.find('\t',p)) != std::string::npos) { fields.push_back(line.substr(p,n-p)); p=n+1; } fields.push_back(line.substr(p)); if(fields.size()<5 || fields[0].empty()) continue;
            try { PluginCompatibilityRecord r{fields[0], fields[1]=="1", static_cast<uint32_t>(std::stoul(fields[2])), static_cast<uint32_t>(std::stoul(fields[3])), fields[4]}; next[r.identifier]=std::move(r); } catch (...) { return false; } }
        m_records.swap(next); return true;
    }
private: std::unordered_map<std::string, PluginCompatibilityRecord> m_records;
};
}
