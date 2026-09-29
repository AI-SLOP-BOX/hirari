#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins {

/// C++ compatibility handle for the Rust-owned bounded SysEx/MIDI reassembler.
class MidiFragmentReassembler final {
public:
    static constexpr std::size_t kFragmentPayloadBytes = 240;
    static constexpr std::size_t kMaximumMessageBytes = 1024u * 1024u;

    struct Fragment {
        uint32_t messageId = 0;
        uint16_t index = 0;
        uint16_t total = 0;
        uint64_t sampleOffset = 0;
        uint8_t articulationId = 0;
        uint16_t size = 0;
        std::array<uint8_t, kFragmentPayloadBytes> payload{};
    };

    enum class Result : uint8_t {
        Accepted,
        Complete,
        Invalid,
        OutOfOrder,
        Oversize,
    };

    MidiFragmentReassembler() : m_state(hirari_midi_fragment_reassembler_create()) {}
    ~MidiFragmentReassembler() { hirari_midi_fragment_reassembler_destroy(m_state); }
    MidiFragmentReassembler(const MidiFragmentReassembler&) = delete;
    MidiFragmentReassembler& operator=(const MidiFragmentReassembler&) = delete;

    void reset() noexcept { hirari_midi_fragment_reassembler_reset(m_state); }

    Result push(const Fragment& fragment) noexcept {
        return static_cast<Result>(hirari_midi_fragment_reassembler_push(
            m_state, fragment.messageId, fragment.index, fragment.total,
            fragment.sampleOffset, fragment.articulationId, fragment.payload.data(), fragment.size));
    }

    bool complete() const noexcept { return hirari_midi_fragment_reassembler_complete(m_state); }
    std::size_t size() const noexcept { return hirari_midi_fragment_reassembler_size(m_state); }
    uint32_t messageId() const noexcept { return hirari_midi_fragment_reassembler_message_id(m_state); }
    uint64_t sampleOffset() const noexcept { return hirari_midi_fragment_reassembler_sample_offset(m_state); }
    uint8_t articulationId() const noexcept { return hirari_midi_fragment_reassembler_articulation_id(m_state); }
    const uint8_t* data() const noexcept { return hirari_midi_fragment_reassembler_data(m_state); }

private:
    void* m_state = nullptr;
};

/// SPSC bounded transport for completed SysEx/MIDI 2.0 messages that cannot
/// fit in MidiEvent's legacy 256-byte slot. The object is trivially placeable
/// in shared memory; no pointers or process-local ownership are stored.
class MidiExtendedMessageRing final {
public:
    static constexpr std::size_t kCapacity = 4;
    static constexpr std::size_t kMaximumMessageBytes =
        MidiFragmentReassembler::kMaximumMessageBytes;
    // Rust's repr(C, align(64)) ring state: four fixed message slots followed
    // by separate cache lines for producer and consumer sequence counters.
    static constexpr std::size_t kStorageBytes =
        kCapacity * (16 + kMaximumMessageBytes) + 2 * 64;

    struct Message {
        uint64_t sampleOffset = 0;
        uint8_t articulationId = 0;
        uint32_t size = 0;
        std::array<uint8_t, kMaximumMessageBytes> data{};
    };

    MidiExtendedMessageRing() noexcept {
        hirari_midi_extended_ring_init(m_storage.data(), m_storage.size());
    }
    MidiExtendedMessageRing(const MidiExtendedMessageRing&) = delete;
    MidiExtendedMessageRing& operator=(const MidiExtendedMessageRing&) = delete;

    bool push(uint64_t sampleOffset, uint8_t articulationId,
              const uint8_t* bytes, std::size_t size) noexcept {
        return hirari_midi_extended_ring_push(
            m_storage.data(), sampleOffset, articulationId, bytes, size);
    }

    bool pop(Message& destination) noexcept {
        return hirari_midi_extended_ring_pop(m_storage.data(), &destination);
    }

    std::size_t size() const noexcept {
        return hirari_midi_extended_ring_size(m_storage.data());
    }

private:
    alignas(64) std::array<std::byte, kStorageBytes> m_storage{};
};

static_assert(sizeof(MidiExtendedMessageRing::Message) ==
              16 + MidiExtendedMessageRing::kMaximumMessageBytes);
static_assert(sizeof(MidiExtendedMessageRing) == MidiExtendedMessageRing::kStorageBytes);

} // namespace Hirari::Core::Plugins
