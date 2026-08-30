#pragma once

#include <string_view>
#include <cstdint>

namespace Aura::Core::Utils {

/**
 * @brief StringHash: Compile-time and Runtime FNV-1a Hash generator.
 * Eliminates the need for std::string comparison in real-time threads.
 */
struct StringHash {
    static constexpr uint32_t OffsetBasis = 2166136261u;
    static constexpr uint32_t Prime = 16777619u;

    /**
     * @brief Computes a unique 32-bit ID for a given string.
     */
    static constexpr uint32_t get(std::string_view str) {
        uint32_t hash = OffsetBasis;
        for (char c : str) {
            hash ^= static_cast<uint32_t>(c);
            hash *= Prime;
        }
        return hash;
    }
};

/**
 * @brief User-defined literal for easy hash creation: "my_track"_id
 */
constexpr uint32_t operator""_id(const char* str, size_t size) {
    return StringHash::get(std::string_view(str, size));
}

} // namespace Aura::Core::Utils
