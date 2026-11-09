#pragma once

// Shared-memory mailbox layout mirror for the VST3 sandbox adapter.
//
// This header describes exactly the bytes the Rust worker's
// `SharedAudioBlock` (hirari-plugin-protocol/src/mailbox.rs) owns: field
// order, atomic sizes and array extents are copied from that single source
// of truth, and the static_asserts below pin the same offsets the Rust side
// asserts, so a layout change on either side fails to compile.

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace Hirari::Core::Plugins::SandboxProtocol {

inline constexpr uint8_t kStateErrorNone = 0u;
inline constexpr uint8_t kStateErrorOversize = 1u;
inline constexpr uint8_t kStateErrorVersion = 2u;
inline constexpr uint8_t kStateErrorChecksum = 3u;
inline constexpr uint8_t kStateErrorPlugin = 4u;
inline constexpr uint8_t kStateErrorUnavailable = 5u;
inline constexpr uint8_t kStateErrorBusy = 6u;
inline constexpr uint8_t kStateErrorTimeout = 7u;

inline constexpr uint32_t kMaxFrames = 8192;
inline constexpr uint32_t kMaxChannels = 2;
inline constexpr uint32_t kMaxMidiEvents = 1024;
inline constexpr uint32_t kMaxParameterChanges = 4096;
inline constexpr uint32_t kMaxMidiPayloadBytes = 256;
inline constexpr uint32_t kMaxStateBytes = 4u * 1024u * 1024u;
inline constexpr uint32_t kStateProtocolVersion = 1u;
inline constexpr uint32_t kAudioBlockProtocolVersion = 6u;
// RING_STORAGE_BYTES in mailbox.rs: 4 slots * (16 + 1024*1024) + 128.
inline constexpr std::size_t kRingStorageBytes = 4u * (16u + 1024u * 1024u) + 128u;

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

static_assert(sizeof(MidiEvent) == 272);
static_assert(offsetof(MidiEvent, data) == 12);
static_assert(offsetof(MidiEvent, articulationId) == 268);
static_assert(sizeof(ParameterChange) == 16);

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
    std::atomic<uint8_t> midiInstrument{0};
    std::atomic<uint64_t> stateRequestSequence{0};
    std::atomic<uint64_t> stateCompletedSequence{0};
    std::atomic<uint64_t> stateProjectGeneration{0};
    std::atomic<uint64_t> statePluginGeneration{0};
    std::atomic<uint64_t> stateAudioGeneration{0};
    std::atomic<uint64_t> stateGeneration{0};
    std::atomic<uint32_t> stateSize{0};
    std::atomic<uint64_t> stateChecksum{0};
    std::atomic<uint32_t> stateVersion{0};
    std::atomic<uint8_t> stateMode{0};
    std::atomic<uint8_t> stateResult{0};
    std::atomic<uint8_t> stateError{kStateErrorNone};
    alignas(64) uint8_t extendedMidi[kRingStorageBytes]{};
    alignas(64) float input[kMaxChannels][kMaxFrames]{};
    alignas(64) float output[kMaxChannels][kMaxFrames]{};
    MidiEvent midi[kMaxMidiEvents]{};
    ParameterChange parameterChange[kMaxParameterChanges]{};
    MidiEvent outputMidi[kMaxMidiEvents]{};
    alignas(64) uint8_t state[kMaxStateBytes]{};
    std::atomic<uint32_t> pluginLatencySamples{0};
};

static_assert(offsetof(SharedAudioBlock, requestSequence) == 8);
static_assert(offsetof(SharedAudioBlock, heartbeat) == 24);
static_assert(offsetof(SharedAudioBlock, frames) == 32);
static_assert(offsetof(SharedAudioBlock, processErrors) == 72);
static_assert(offsetof(SharedAudioBlock, stateRequestSequence) == 80);
static_assert(offsetof(SharedAudioBlock, stateSize) == 128);
static_assert(offsetof(SharedAudioBlock, stateChecksum) == 136);
static_assert(offsetof(SharedAudioBlock, stateError) == 150);
static_assert(offsetof(SharedAudioBlock, extendedMidi) == 192);
static_assert(offsetof(SharedAudioBlock, input) == 4'194'688);
static_assert(offsetof(SharedAudioBlock, output) == 4'260'224);
static_assert(offsetof(SharedAudioBlock, midi) == 4'325'760);
static_assert(offsetof(SharedAudioBlock, parameterChange) == 4'604'288);
static_assert(offsetof(SharedAudioBlock, outputMidi) == 4'669'824);
static_assert(offsetof(SharedAudioBlock, state) == 4'948'352);
static_assert(offsetof(SharedAudioBlock, pluginLatencySamples) == 9'142'656);
static_assert(offsetof(SharedAudioBlock, input) % 64 == 0);
static_assert(offsetof(SharedAudioBlock, extendedMidi) % 64 == 0);

} // namespace Hirari::Core::Plugins::SandboxProtocol
