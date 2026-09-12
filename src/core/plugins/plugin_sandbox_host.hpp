#pragma once
#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <mutex>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <new>
#include <chrono>
#include <limits>
#include <cstdlib>
#include <cctype>
#include <filesystem>
#if !defined(_WIN32)
#include <cerrno>
#include <csignal>
#include <fcntl.h>
#include <poll.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <unistd.h>
#include <spawn.h>
#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

#if !defined(_WIN32)
extern char** environ;
#endif
#endif
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "plugin_sandbox_protocol.hpp"
#include "plugin_admission.hpp"

namespace Aura::Core::Plugins {

/**
 * @class PluginSandboxHost
 * @brief Process-lifecycle host for isolated third-party plugins.
 *
 * The helper is a separate OS process. The parent never loads the plugin
 * binary, and it will not expose a live state until the helper acknowledges
 * initialization over the control pipe.
 */
class PluginSandboxHost {
#include "plugin_sandbox_host_part_1.inc"
#include "plugin_sandbox_host_part_2.inc"
#include "plugin_sandbox_host_part_3.inc"
#include "plugin_sandbox_host_part_4.inc"
#include "plugin_sandbox_host_part_5.inc"
#include "plugin_sandbox_host_part_6.inc"
#include "plugin_sandbox_host_part_7.inc"
#include "plugin_sandbox_host_part_8.inc"
};

} // namespace Aura::Core::Plugins
