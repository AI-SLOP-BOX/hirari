// The ARUA project codec is Rust-owned; this generated header exposes the cxx
// bridge types the contract uses to round-trip a project container.
#include "hirari-core-bridge/src/lib.rs.h"
#include "src/core/project_automation.hpp"
#include "src/core/engine/track_freeze_manager.hpp"
#include "src/core/rust_ffi.hpp"
#include <atomic>
#include <array>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <limits>
#include <string>
#include <thread>
#include <utility>

// The SDKContractFactory/Processor and UndoContractCommand scaffolding that
// used to live here covered the duplicate C++ plugin SDK and the C++ undo
// manager, both since deleted. Descriptor validation, state cache identity and
// automation interpolation are Rust-owned and tested in plugin_parameters.rs,
// plugin_catalog.rs and realtime_automation.rs; transactional undo is covered
// against the Rust manager in native_plugin_compile_contract_part_1.inc.

#include "native_plugin_compile_contract_part_1.inc"
#include "native_plugin_compile_contract_part_2.inc"
#include "native_plugin_compile_contract_part_3.inc"
