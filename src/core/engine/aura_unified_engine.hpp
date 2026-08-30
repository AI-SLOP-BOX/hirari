#pragma once

// Compatibility path for older native integrations.  The real implementation
// lives one level up; keeping this forwarding header prevents the historical
// empty renderBlock() stub from being selected accidentally.
#include "../aura_unified_engine.hpp"
