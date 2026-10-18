#include <cassert>
#include <filesystem>
#include <fstream>

#include "../src/io/persistence/async_serializer.hpp"

int main() {
    // The AutomationManager block that opened this test moved to
    // `automation_manager::tests::assertions_moved_from_the_cpp_contract_tests_hold_without_the_facade`.
    // `src/core/engine/automation_manager.hpp` only forwarded to the Rust-owned
    // `hirari_automation_manager_*` symbols and no production code included it.

    const auto root = std::filesystem::temp_directory_path() / "hirari-serializer-contract";
    const auto path = root / "nested" / "project.json";
    auto future = Hirari::IO::Persistence::AsyncSerializer::getInstance()
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
