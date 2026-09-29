#pragma once
#include <vector>
#include <string>

/**
 * @def HIRARI_INDUSTRIAL_SHIM
 * @brief RADICAL BOILERPLATE OMISSION: Unified meta-programming for Rust shims.
 * 
 * This macro replaces the standard "INDUSTRIAL TRANSITION" comments with a 
 * clean, technical, and high-performance dispatch mechanism.
 */
#include <cassert>
#include "diagnostics_kernel.hpp"

#define HIRARI_INDUSTRIAL_SHIM(ClassName, BridgeNamespace, RustEngineName) \
    /** \
     * @brief INDUSTRIAL DISPATCH: Delegating execution to the Rust sovereign core. \
     */ \
    void process(float* l, float* r, size_t numFrames) { \
        assert(l != nullptr && "Left channel pointer cannot be null"); \
        assert(r != nullptr && "Right channel pointer cannot be null"); \
        Hirari::Core::Bridge::BridgeNamespace::process_##RustEngineName(l, r, numFrames); \
    } \
    \
    /** \
     * @brief FORENSIC AUDIT: Verifying the integrity of the Rust-powered terminal state. \
     */ \
    bool audit() const { \
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "AUDIT_" #ClassName); \
        return Hirari::Core::Bridge::BridgeNamespace::audit_##RustEngineName(); \
    }

namespace Hirari::Core::Bridge {
    // Shared bridge utilities can go here
}
