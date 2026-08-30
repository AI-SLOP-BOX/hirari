#include <cassert>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <thread>
#include "io/persistence/async_serializer.hpp"

int main() {
    const auto path = std::filesystem::temp_directory_path() /
        ("aura-serializer-" + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()) + ".json");
    auto& serializer = Aura::IO::Persistence::AsyncSerializer::getInstance();
    auto result = serializer.serializeAsync(path.string(), "{\"ok\":true}");
    assert(result.valid());
    assert(result.get());

    std::ifstream input(path, std::ios::binary);
    std::string contents((std::istreambuf_iterator<char>(input)), {});
    assert(contents == "{\"ok\":true}");
    assert(!std::filesystem::exists(path.string() + ".tmp"));

    auto invalid = serializer.serializeAsync("", "data");
    assert(invalid.valid());
    assert(!invalid.get());
    std::filesystem::remove(path);
    return 0;
}
