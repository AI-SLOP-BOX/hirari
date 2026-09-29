#pragma once

// Compatibility entry point kept for older integrations.
//
// This header used to contain a second, incomplete MasterSuite implementation
// whose EQ, convolution and neural stages were silent no-ops.  Keeping two
// classes with the same public role made it possible for callers to bypass the
// real mastering path accidentally.  The product implementation is now
// single-sourced through master_suite.hpp.
#include "master_suite.hpp"

namespace Hirari::DSP::Mixing {

// Source compatibility for integrations that used the old header.  This alias
// deliberately resolves to the real IProcessor-backed implementation.
using HirariMasterSuite = MasterSuite;

} // namespace Hirari::DSP::Mixing
