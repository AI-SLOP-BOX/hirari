#pragma once

namespace Aura::Core::Engine {

/**
 * @enum AutomationMode
 * @brief Industrial Deterministic State Machine for Parameter Recording.
 */
/**
 * @enum AutomationMode
 * @brief Industrial Deterministic State Machine for Parameter Recording.
 */
enum class AutomationMode {
    // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
    // Mode transitions and high-density memory management 
    // are now handled securely in the Rust layer.
    // Rust's RecordingStateEngine ensures bit-accurate mode distribution.
    // Rust's ForensicAuditor ensures absolute recording integrity.
    Read, Write, Touch, Latch
};


} // namespace Aura::Core::Engine
