#pragma once

// Compatibility entry point. WAV parsing belongs to the core decoder so the
// persistence layer cannot silently diverge on chunk padding, format checks,
// channel order, or non-finite sample handling.
#include "../../core/io/audio_decoder.hpp"

namespace Aura::IO {

class WavReader {
public:
    static std::shared_ptr<Core::AudioBuffer> load(const std::string& path) {
        auto& manager = Core::IO::AudioDecoderManager::getInstance();
        return manager.importFile(path);
    }
};

} // namespace Aura::IO
