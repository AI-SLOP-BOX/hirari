#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <algorithm>

namespace Aura::Core::Plugins {

/// Bounded native transport for SysEx and other MIDI payloads that do not fit
/// in the legacy 256-byte realtime event slot. It deliberately owns no heap
/// memory and never accepts out-of-order or mixed-message fragments.
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

    void reset() noexcept {
        m_messageId = 0;
        m_nextIndex = 0;
        m_total = 0;
        m_size = 0;
        m_sampleOffset = 0;
        m_articulationId = 0;
        m_complete = false;
    }

    Result push(const Fragment& fragment) noexcept {
        const bool shapeValid = fragment.total != 0 &&
            fragment.index < fragment.total && fragment.size != 0 &&
            fragment.size <= kFragmentPayloadBytes;
        if (!shapeValid) { reset(); return Result::Invalid; }

        if (fragment.index == 0) {
            reset();
            m_messageId = fragment.messageId;
            m_total = fragment.total;
            m_sampleOffset = fragment.sampleOffset;
            m_articulationId = fragment.articulationId;
        }
        if (fragment.messageId != m_messageId || fragment.total != m_total ||
            fragment.index != m_nextIndex) {
            reset();
            return Result::OutOfOrder;
        }
        if (m_size > kMaximumMessageBytes - fragment.size) {
            reset();
            return Result::Oversize;
        }
        std::copy_n(fragment.payload.data(), fragment.size, m_bytes.data() + m_size);
        m_size += fragment.size;
        ++m_nextIndex;
        if (m_nextIndex == m_total) {
            m_complete = true;
            return Result::Complete;
        }
        return Result::Accepted;
    }

    bool complete() const noexcept { return m_complete; }
    std::size_t size() const noexcept { return m_size; }
    uint32_t messageId() const noexcept { return m_messageId; }
    uint64_t sampleOffset() const noexcept { return m_sampleOffset; }
    uint8_t articulationId() const noexcept { return m_articulationId; }
    const uint8_t* data() const noexcept { return m_bytes.data(); }

private:
    uint32_t m_messageId = 0;
    uint16_t m_nextIndex = 0;
    uint16_t m_total = 0;
    std::size_t m_size = 0;
    uint64_t m_sampleOffset = 0;
    uint8_t m_articulationId = 0;
    bool m_complete = false;
    std::array<uint8_t, kMaximumMessageBytes> m_bytes{};
};

/// SPSC bounded transport for completed SysEx/MIDI 2.0 messages that cannot
/// fit in MidiEvent's legacy 256-byte slot. The object is trivially placeable
/// in shared memory; no pointers or process-local ownership are stored.
class MidiExtendedMessageRing final {
public:
    static constexpr std::size_t kCapacity = 4;
    static constexpr std::size_t kMaximumMessageBytes =
        MidiFragmentReassembler::kMaximumMessageBytes;

    struct Message {
        uint64_t sampleOffset = 0;
        uint8_t articulationId = 0;
        uint32_t size = 0;
        std::array<uint8_t, kMaximumMessageBytes> data{};
    };

    bool push(uint64_t sampleOffset, uint8_t articulationId,
              const uint8_t* bytes, std::size_t size) noexcept {
        if (bytes == nullptr || size == 0 || size > kMaximumMessageBytes) return false;
        const auto head = m_head.load(std::memory_order_relaxed);
        const auto tail = m_tail.load(std::memory_order_acquire);
        if (head - tail >= kCapacity) return false;
        auto& slot = m_slots[head % kCapacity];
        std::copy_n(bytes, size, slot.data.data());
        slot.sampleOffset = sampleOffset;
        slot.articulationId = articulationId;
        slot.size = static_cast<uint32_t>(size);
        m_head.store(head + 1, std::memory_order_release);
        return true;
    }

    bool pop(Message& destination) noexcept {
        const auto tail = m_tail.load(std::memory_order_relaxed);
        const auto head = m_head.load(std::memory_order_acquire);
        if (tail == head) return false;
        destination = m_slots[tail % kCapacity];
        m_tail.store(tail + 1, std::memory_order_release);
        return true;
    }

    std::size_t size() const noexcept {
        return static_cast<std::size_t>(m_head.load(std::memory_order_acquire) -
                                        m_tail.load(std::memory_order_acquire));
    }

private:
    std::array<Message, kCapacity> m_slots{};
    alignas(64) std::atomic<uint64_t> m_head{0};
    alignas(64) std::atomic<uint64_t> m_tail{0};
};

} // namespace Aura::Core::Plugins
