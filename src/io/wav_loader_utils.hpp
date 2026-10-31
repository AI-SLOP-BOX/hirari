#pragma once

#include <vector>
#include <memory>
#include <string>
#include <cstdint>
#include "../core/rust_ffi.hpp"

namespace Hirari::IO {

/**
 * @brief WavLoader: WAVE file metadata and diagnostics.
 * Decoding, normalisation and failure wording all live in Rust; this only
 * forwards a path and copies the returned JSON bytes.
 */
class WavLoader {
public:
    /// Stable JSON-shaped diagnostic for FFI and native contract callers.
    static std::string loadDiagnosticJson(const std::string& path,
                                          bool wave64 = false) noexcept {
        void* raw = hirari_wav_diagnostic_json_for_path(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), wave64);
        if (!raw) return {};
        std::unique_ptr<char, decltype(&hirari_wav_diagnostic_json_free)>
            json(static_cast<char*>(raw), &hirari_wav_diagnostic_json_free);
        const auto* data = hirari_wav_diagnostic_json_data(json.get());
        const size_t size = hirari_wav_diagnostic_json_size(json.get());
        return data ? std::string(reinterpret_cast<const char*>(data), size) : std::string();
    }
};

} // namespace Hirari::IO
