#pragma once
#include <cstdint>
#include <vector>
#include <algorithm>
#include <array>

namespace Aura::Core::Midi {

/**
 * @struct UniversalMidiPacket
 * @brief Represents a MIDI 2.0 message packet.
 */
struct UniversalMidiPacket {
    uint32_t data[4]; // 32, 64, 96, or 128-bit packets
};

/**
 * @class MIDI2Kernel
 * @brief High-density MIDI 2.0 processing engine.
 */
class MIDI2Kernel {
public:
    struct DecodedMidiEvent {
        uint8_t status;
        uint8_t channel;
        uint8_t data1;
        uint8_t data2;
        uint16_t value16; // 16-bit extended velocity / CC value for MIDI 2.0
    };

    static MIDI2Kernel& getInstance() {
        static MIDI2Kernel instance;
        return instance;
    }

    // Realtime-safe entry point: callers provide the destination storage so
    // UMP decoding never allocates on the audio/MIDI thread.
    std::size_t processPacketsInto(const UniversalMidiPacket* packets,
                                   std::size_t packetCount,
                                   DecodedMidiEvent* output,
                                   std::size_t outputCapacity) const noexcept {
        if (!packets || !output || outputCapacity == 0) return 0;
        const std::size_t limit = std::min(packetCount, outputCapacity);
        std::size_t written = 0;
        for (std::size_t index = 0; index < limit; ++index) {
            const uint32_t w0 = packets[index].data[0];
            const uint8_t mt = static_cast<uint8_t>((w0 >> 28) & 0x0F);
            const uint8_t status = static_cast<uint8_t>((w0 >> 16) & 0xF0);
            if (status < 0x80 || status > 0xE0) continue;
            const uint8_t channel = static_cast<uint8_t>((w0 >> 16) & 0x0F);
            const uint8_t data1 = static_cast<uint8_t>((w0 >> 8) & 0x7F);
            if (mt == 0x2) {
                output[written++] = {status, channel, data1,
                                     static_cast<uint8_t>(w0 & 0x7F),
                                     static_cast<uint16_t>(w0 & 0x7F)};
            } else if (mt == 0x4) {
                const uint16_t value = static_cast<uint16_t>((packets[index].data[1] >> 16) & 0xFFFF);
                const uint8_t data2 = static_cast<uint8_t>(std::min<uint32_t>(127u,
                    (static_cast<uint32_t>(value) * 127u + 32767u) / 65535u));
                output[written++] = {status, channel, data1, data2, value};
            }
        }
        return written;
    }

    /**
     * @brief Processes a batch of Universal MIDI Packets and returns decoded events.
     */
    std::vector<DecodedMidiEvent> processPackets(const std::vector<UniversalMidiPacket>& packets) {
        std::vector<DecodedMidiEvent> events;
        events.reserve(std::min<size_t>(packets.size(), 4096));
        std::array<DecodedMidiEvent, 4096> decoded{};
        const std::size_t count = processPacketsInto(packets.data(), packets.size(),
                                                     decoded.data(), decoded.size());
        events.insert(events.end(), decoded.begin(), decoded.begin() + count);
        return events;
    }

private:
    MIDI2Kernel() = default;
};

} // namespace Aura::Core::Midi
