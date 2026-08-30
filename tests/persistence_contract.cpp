#include "src/io/persistence/async_serializer.hpp"

#include <cassert>
#include <filesystem>
#include <string>
#include <thread>
#include <chrono>
#if !defined(_WIN32)
#include <unistd.h>
#endif

int main() {
#if defined(_WIN32)
    const unsigned pid = 0u;
#else
    const unsigned pid = static_cast<unsigned>(::getpid());
#endif
    const auto path = std::filesystem::temp_directory_path() /
        ("aura-persistence-contract-" + std::to_string(pid) + ".json");
    const auto lock = std::filesystem::path(path.string() + ".save.lock");
    std::error_code ignored;
    std::filesystem::remove(path, ignored);
    std::filesystem::remove(lock, ignored);

    auto& serializer = Aura::IO::Persistence::AsyncSerializer::getInstance();
    auto first = serializer.serializeAsync(path.string(), "{\"version\":1}");
    auto second = serializer.serializeAsync(path.string(), "{\"version\":2}");
    assert(first.get());
    assert(!second.get());
    assert(std::filesystem::is_regular_file(path));
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
    while (std::filesystem::exists(lock) && std::chrono::steady_clock::now() < deadline) {
        std::this_thread::yield();
    }
    assert(!std::filesystem::exists(lock));

    std::filesystem::remove(path, ignored);
    return 0;
}
