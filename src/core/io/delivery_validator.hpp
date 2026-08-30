#pragma once
#include <filesystem>
#include <string>
#include <vector>
#include <fstream>

namespace Aura::Core::IO {
struct DeliveryIssue { std::filesystem::path file; std::string message; };
struct DeliveryReport { size_t checked=0; std::vector<DeliveryIssue> issues; bool ok() const noexcept{return issues.empty()&&checked>0;} };
class DeliveryValidator {
public:
    static DeliveryReport validate(const std::vector<std::filesystem::path>& files, uintmax_t maxBytes=0) {
        DeliveryReport r;
        for(const auto& p:files){++r.checked;std::error_code ec; if(!std::filesystem::is_regular_file(p,ec)||ec){r.issues.push_back({p,"missing or not a regular file"});continue;} const auto n=std::filesystem::file_size(p,ec); if(ec||n==0){r.issues.push_back({p,"empty or unreadable"});continue;} if(maxBytes&&n>maxBytes)r.issues.push_back({p,"exceeds size limit"}); const auto ext=p.extension().string(); if(ext.empty())r.issues.push_back({p,"missing extension"}); }
        return r;
    }
    static bool writeManifest(const std::filesystem::path& path,const DeliveryReport& report){std::ofstream f(path,std::ios::trunc);if(!f)return false;f<<"file\tstatus\tmessage\n";for(const auto&i:report.issues)f<<i.file.string()<<"\tFAIL\t"<<i.message<<"\n";f.flush();return static_cast<bool>(f);}
};
}
