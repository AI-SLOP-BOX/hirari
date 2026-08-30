#include <cassert>
#include <filesystem>
#include <fstream>
#include <limits>

#include "../src/core/engine/automation_manager.hpp"
#include "../src/io/persistence/async_serializer.hpp"

int main() {
    auto& automation = Aura::Core::Engine::AutomationManager::getInstance();
    automation.reset();
    assert(automation.setTarget(2, 4, 0.75f));
    assert(!automation.setTarget(1024, 0, 1.0f));
    assert(!automation.setTarget(0, 0, std::numeric_limits<float>::quiet_NaN()));
    assert(automation.getTarget(2, 4) == 0.75f);

    const auto root = std::filesystem::temp_directory_path() / "aura-serializer-contract";
    const auto path = root / "nested" / "project.json";
    auto future = Aura::IO::Persistence::AsyncSerializer::getInstance()
                      .serializeAsync(path.string(), "{\"ok\":true}");
    assert(future.valid() && future.get());
    std::ifstream file(path, std::ios::binary);
    assert(file.good());
    std::string content((std::istreambuf_iterator<char>(file)), {});
    assert(content == "{\"ok\":true}");
    std::error_code ec;
    std::filesystem::remove_all(root, ec);
    return 0;
}
