#pragma once
#include "rust/cxx.h"
#include <string>

namespace Aura::Core::BridgeFFI {

// --- HARDENING: Non-blocking Bridge API ---
// Forward declarations for CXX generated headers
class AudioEngine;
class ProjectManager;
class AnalysisHub;
class GpuResourceManager;

// Static logging trigger for C++ side
void AURA_LOG(uint32_t level, const std::string& msg);

} // namespace Aura::Core::Bridge
