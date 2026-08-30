#pragma once
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

namespace Aura::Core::Engine {
class CrashRecoveryStore {
public:
    explicit CrashRecoveryStore(std::filesystem::path path):m_path(std::move(path)){}
    bool write(const std::vector<uint8_t>& data){if(data.empty())return false;const auto t=m_path.string()+".recovery.tmp";std::ofstream f(t,std::ios::binary|std::ios::trunc);if(!f)return false;f.write(reinterpret_cast<const char*>(data.data()),data.size());f.flush();if(!f)return false;std::error_code ec;std::filesystem::rename(t,m_path,ec);if(ec)std::filesystem::remove(t,ec);return !ec;}
    bool available()const{std::error_code ec;return std::filesystem::is_regular_file(m_path,ec)&&!ec&&std::filesystem::file_size(m_path,ec)>0&&!ec;}
    bool read(std::vector<uint8_t>& data)const{if(!available())return false;std::ifstream f(m_path,std::ios::binary);if(!f)return false;data.assign(std::istreambuf_iterator<char>(f),{});return !data.empty();}
    bool discard(){std::error_code ec;std::filesystem::remove(m_path,ec);return !ec;}
private:std::filesystem::path m_path;
};
}
