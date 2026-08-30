#include "audio_interface.hpp"

namespace Aura::IO {

std::unique_ptr<IAudioInterface> HardwareFactory::createDefault() {
    return std::make_unique<LegacyDriverAdapter>(
        Drivers::DriverFactory::create(Drivers::DriverFactory::API::Auto));
}

} // namespace Aura::IO
