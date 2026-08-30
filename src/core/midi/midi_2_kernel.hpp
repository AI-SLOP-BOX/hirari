#pragma once
#include <cstdint>
#include <vector>

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

    /**
     * @brief Processes a batch of Universal MIDI Packets and returns decoded events.
     */
    std::vector<DecodedMidiEvent> processPackets(const std::vector<UniversalMidiPacket>& packets) {
        std::vector<DecodedMidiEvent> events;
        events.reserve(packets.size());
        
        for (const auto& packet : packets) {
            uint32_t w0 = packet.data[0];
            uint8_t mt = (w0 >> 28) & 0x0F;
            
            if (mt == 0x2) { // MIDI 1.0 Channel Voice over UMP (64-bit)
                uint8_t status = (w0 >> 16) & 0xF0;
                uint8_t channel = (w0 >> 16) & 0x0F;
                uint8_t d1 = (w0 >> 8) & 0x7F;
                uint8_t d2 = w0 & 0x7F;
                events.push_back({status, channel, d1, d2, static_cast<uint16_t>(d2)});
            }
            else if (mt == 0x4) { // MIDI 2.0 Channel Voice (64-bit)
                uint8_t status = (w0 >> 16) & 0xF0;
                uint8_t channel = (w0 >> 16) & 0x0F;
                uint8_t d1 = (w0 >> 8) & 0x7F; // Note or Index
                uint32_t w1 = packet.data[1];
                uint16_t val16 = (w1 >> 16) & 0xFFFF; // 16-bit high-resolution velocity/value
                uint8_t d2 = static_cast<uint8_t>(val16 >> 9); // Downscale to 7-bit for compatibility
                events.push_back({status, channel, d1, d2, val16});
            }
        }
        return events;
    }

private:
    MIDI2Kernel() = default;
};

} // namespace Aura::Core::Midi
