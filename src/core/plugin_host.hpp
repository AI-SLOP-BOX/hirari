#pragma once
#include <string>
#include <vector>
#include <memory>
#include <atomic>
#include <array>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <mutex>
#include "../dsp/iprocessor.hpp"
#include "../dsp/effects/pro_limiter.hpp"
#include "../dsp/effects/sub_bass_generator.hpp"
#include "../dsp/effects/compressor.hpp"
#include "../dsp/effects/tube_saturation.hpp"
#include "../scae/AuraAISuite.hpp"
#include "plugins/process_sandbox_processor.hpp"
#include "plugins/vst3_host_processor.hpp"

namespace Aura::Core::Plugin {
#include "plugin_host_part_1.inc"
#include "plugin_host_part_2.inc"
#include "plugin_host_part_3.inc"
} // namespace Aura::Core::Plugin
