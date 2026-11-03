#pragma once

#include <cstddef>
#include <cstdint>

namespace Hirari::Core::Plugins::SandboxProtocol {

// The shared-memory mailbox is Rust-owned (`hirari-plugin-protocol/src/mailbox.rs`);
// C++ only needs the numeric bounds it enforces before handing data to the host.
inline constexpr uint32_t kMaxFrames = 8192;
inline constexpr uint32_t kMaxChannels = 2;
inline constexpr uint32_t kMaxParameterChanges = 4096;
inline constexpr uint32_t kMaxMidiPayloadBytes = 256;

// Recovery modes mirror `SandboxRecovery` in the Rust host; the C++ processor
// maps them onto its own enum so a stalled or quarantined slot stays visible.
inline constexpr uint8_t kRecoveryClearBlock = 0u;
inline constexpr uint8_t kRecoveryQuarantined = 1u;

// State error surfaced when a slot cannot serve a state request. The Rust
// worker owns the remaining result codes.
inline constexpr uint8_t kStateErrorUnavailable = 5u;

} // namespace Hirari::Core::Plugins::SandboxProtocol
