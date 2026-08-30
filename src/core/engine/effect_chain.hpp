#pragma once

// Keep the historical Engine namespace source-compatible while using the
// single functional EffectChain implementation. The previous class was a
// no-op Rust-bridge shim, which silently discarded every inserted processor.
#include "../effect_chain.hpp"

namespace Aura::Core::Engine {
using EffectChain = ::Aura::Core::EffectChain;
}
