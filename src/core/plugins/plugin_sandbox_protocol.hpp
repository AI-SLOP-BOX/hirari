#pragma once

#include <array>
#include <cstdint>
#include <atomic>
#include "midi_fragment_transport.hpp"

namespace Aura::Core::Plugins::SandboxProtocol {

inline constexpr uint8_t kReady = 0xA1;
inline constexpr uint8_t kShutdown = 0xA2;
inline constexpr uint8_t kHeartbeat = 0xA3;
inline constexpr uint8_t kReset = 0xA4;
inline constexpr uint8_t kError = 0xAF;
inline constexpr uint8_t kErrorLoad = 0xB0;
inline constexpr uint8_t kErrorAbi = 0xB1;
inline constexpr uint8_t kErrorInstance = 0xB2;
inline constexpr uint8_t kErrorSecurity = 0xB3;
inline constexpr uint8_t kErrorUnsupported = 0xB4;
inline constexpr uint32_t kMaxFrames = 8192;
inline constexpr uint32_t kMaxChannels = 2;
inline constexpr uint32_t kMaxMidiEvents = 1024;
inline constexpr uint32_t kMaxParameterChanges = 256;
inline constexpr uint32_t kMaxMidiPayloadBytes = 256;
inline constexpr uint32_t kMaxStateBytes = 4u * 1024u * 1024u;
inline constexpr uint32_t kStateProtocolVersion = 1u;
// SharedAudioBlock contains the 64-bit state checksum below. Bump the ABI
// version so an older worker cannot interpret the shared layout incorrectly.
inline constexpr uint32_t kAudioBlockProtocolVersion = 3u;
inline constexpr uint8_t kStateErrorNone = 0u;
inline constexpr uint8_t kStateErrorOversize = 1u;
inline constexpr uint8_t kStateErrorVersion = 2u;
inline constexpr uint8_t kStateErrorChecksum = 3u;
inline constexpr uint8_t kStateErrorPlugin = 4u;
inline constexpr uint8_t kStateErrorUnavailable = 5u;
inline constexpr uint8_t kStateErrorBusy = 6u;
inline constexpr uint8_t kStateErrorTimeout = 7u;
inline constexpr uint32_t kStatusHeaderV9 = 0x41555209u;
inline constexpr uint32_t kRecoveryClearBlock = 0u;
inline constexpr uint32_t kRecoveryQuarantined = 1u;

inline uint64_t stateChecksum(const uint8_t* data, size_t size) noexcept {
    uint64_t hash = 14695981039346656037ull;
    for (size_t index = 0; index < size; ++index) {
        hash ^= data[index];
        hash *= 1099511628211ull;
    }
    return hash;
}

// Plugin state is opaque to the host. Until a plugin-specific migration
// callback exists, accepting any other version would silently feed bytes from
// an incompatible schema into the plugin. Keep the policy centralized so
// host and worker cannot drift.
inline bool isSupportedStateVersion(uint32_t version) noexcept {
    return version == kStateProtocolVersion;
}

// Modular comparison remains correct when a uint64 sequence wraps. Never
// compare mailbox sequence numbers with plain >= or <=.
inline bool sequenceReached(uint64_t completed, uint64_t requested) noexcept {
    return static_cast<int64_t>(completed - requested) >= 0;
}

struct MidiEvent {
    uint64_t sampleOffset = 0;
    uint32_t size = 0;
    uint8_t data[kMaxMidiPayloadBytes]{};
    uint8_t articulationId = 0;
};

struct ParameterChange {
    uint32_t parameterId = 0;
    uint32_t sampleOffset = 0;
    double value = 0.0;
};

// This mailbox is inherited across fork and backed by MAP_SHARED memory.
// It is deliberately bounded so the audio callback never allocates or waits.
struct SharedAudioBlock {
    std::atomic<uint32_t> protocolVersion{0};
    std::atomic<uint64_t> requestSequence{0};
    std::atomic<uint64_t> completedSequence{0};
    std::atomic<uint64_t> heartbeat{0};
    std::atomic<uint32_t> frames{0};
    std::atomic<uint32_t> channels{0};
    std::atomic<uint32_t> activeSampleRate{0};
    std::atomic<uint32_t> activeChannels{0};
    std::atomic<uint32_t> midiEvents{0};
    std::atomic<uint32_t> parameterChanges{0};
    std::atomic<uint32_t> outputMidiEvents{0};
    std::atomic<uint32_t> outputMidiDropped{0};
    std::atomic<uint32_t> mailboxOverruns{0};
    std::atomic<uint32_t> inputMidiTruncations{0};
    std::atomic<uint32_t> processErrors{0};
    std::atomic<uint64_t> stateRequestSequence{0};
    std::atomic<uint64_t> stateCompletedSequence{0};
    // State is valid only for the exact project/plugin/audio generation that
    // requested it. These are written before stateRequestSequence is published
    // and read back before the host accepts the acknowledgement.
    std::atomic<uint64_t> stateProjectGeneration{0};
    std::atomic<uint64_t> statePluginGeneration{0};
    std::atomic<uint64_t> stateAudioGeneration{0};
    std::atomic<uint64_t> stateGeneration{0};
    std::atomic<uint32_t> stateSize{0};
    std::atomic<uint64_t> stateChecksum{0};
    std::atomic<uint32_t> stateVersion{0};
    std::atomic<uint8_t> stateMode{0}; // 1 = save, 2 = load
    std::atomic<uint8_t> stateResult{0};
    std::atomic<uint8_t> stateError{kStateErrorNone};
    // Completed SysEx/MIDI 2.0 messages use the bounded ring rather than
    // being silently truncated into the legacy 256-byte event slot.
    MidiExtendedMessageRing extendedMidi;
    alignas(64) float input[kMaxChannels][kMaxFrames]{};
    alignas(64) float output[kMaxChannels][kMaxFrames]{};
    MidiEvent midi[kMaxMidiEvents]{};
    ParameterChange parameterChange[kMaxParameterChanges]{};
    MidiEvent outputMidi[kMaxMidiEvents]{};
    alignas(64) uint8_t state[kMaxStateBytes]{};
};

} // namespace Aura::Core::Plugins::SandboxProtocol
