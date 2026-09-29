#include "audio_interface.hpp"

namespace Hirari::IO {

std::unique_ptr<IAudioInterface> HardwareFactory::createDefault() {
    return std::make_unique<LegacyDriverAdapter>(
        Drivers::DriverFactory::create(Drivers::DriverFactory::API::Auto));
}

} // namespace Hirari::IO
