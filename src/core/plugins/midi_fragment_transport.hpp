#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins {

/// SPSC bounded transport for completed SysEx/MIDI 2.0 messages that cannot
/// fit in MidiEvent's legacy 256-byte slot. The object is trivially placeable
/// in shared memory; no pointers or process-local ownership are stored.
class MidiExtendedMessageRing final {
public:
    static constexpr std::size_t kCapacity = 4;
    static constexpr std::size_t kMaximumMessageBytes = 1024u * 1024u;
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
        // Pass the destination capacity explicitly: the Rust side refuses to
        // copy a full slot into a buffer it cannot verify, which is what turns
        // a caller/ABI size mismatch into a `false` instead of an overflow.
        return hirari_midi_extended_ring_pop(
            m_storage.data(), &destination, sizeof(Message));
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
