#include "midi_orchestrator.hpp"
#include "../midi_buffer.hpp"
#include <algorithm>
#include <cstring>

namespace Aura::Core::Engine {

void MIDIOrchestrator::processMPE(MidiBuffer& buffer) {
    if (m_resetMpe.exchange(false, std::memory_order_acq_rel)) m_mpeNotes.fill({});
    if (!m_mpeEnabled.load(std::memory_order_relaxed)) return;

    // --- MPE NOTE-PER-CHANNEL TRANSLATION ---
    // MPE uses a Master Channel (typically 1 or 16) and Member Channels.
    // Each note is assigned a unique channel for per-note pitch bend, pressure, and timbre.
    
    // (Simulation of high-density MPE mapping logic)
    const MidiEvent* events = buffer.getEvents();
    for (size_t i = 0; i < buffer.size(); ++i) {
        const auto& msg = events[i];
        uint8_t status = msg.data[0] & 0xF0;
        uint8_t channel = msg.data[0] & 0x0F;

        // Channel pressure is the only channel-voice message handled here
        // that is two bytes long. Do not discard valid aftertouch by applying
        // the three-byte note/pitch-bend requirement globally.
        if (status == 0xD0) {
            if (msg.size < 2) continue;
        } else if (msg.size < 3) {
            continue;
        }

        if (status == 0x90 && msg.data[2] > 0) { // Note On
            // Keep one independent state per member channel.  An explicit
            // active flag is required because MIDI note 0 is valid.
            m_mpeNotes[channel] = { true, msg.data[1], channel, 0.0f, 0.0f, 0.0f };
        } else if (status == 0x80 || (status == 0x90 && msg.data[2] == 0)) {
            m_mpeNotes[channel].active = false;
        } else if (status == 0xD0) { // Channel Pressure (Aftertouch)
            if (m_mpeNotes[channel].active) {
                m_mpeNotes[channel].pressure = msg.data[1] / 127.0f;
            }
        } else if (status == 0xE0) { // Pitch Bend
            if (m_mpeNotes[channel].active) {
                uint16_t bend = (msg.data[2] << 7) | msg.data[1];
                m_mpeNotes[channel].bend = (bend - 8192) / 8192.0f;
            }
        }
    }
}

void MIDIOrchestrator::sendSysEx(const SysExBuffer& buffer) {
    if (buffer.data.empty() || buffer.data.size() > 4096) return;
    std::lock_guard<std::mutex> lock(m_sysExProducerMutex);
    (void)m_sysExQueue.push(buffer);
}

void MIDIOrchestrator::handleIncomingSysEx(const uint8_t* data, size_t size) {
    if (!data || size < 3 || size > 4096 || data[0] != 0xF0 || data[size - 1] != 0xF7)
        return;

    // MIDI manufacturer IDs are either one byte or the extended 00 xx yy
    // form. Preserve the complete identity instead of collapsing it to the
    // first byte, which made vendor-specific handshakes indistinguishable.
    uint32_t manufacturerId = data[1];
    if (data[1] == 0x00 && size >= 5)
        manufacturerId = (static_cast<uint32_t>(data[2]) << 8) | data[3];

    SysExBuffer message;
    message.manufacturerId = manufacturerId;
    message.data.assign(data, data + size);
    std::lock_guard<std::mutex> lock(m_sysExProducerMutex);
    (void)m_sysExQueue.push(message);
}

bool MIDIOrchestrator::tryPopSysEx(SysExBuffer& buffer) noexcept {
    return m_sysExQueue.pop(buffer);
}

void MIDIOrchestrator::setArticulationMap(const std::vector<ArticulationMap>& maps) {
    uint32_t next = 1 - m_activeArtIdx.load(std::memory_order_relaxed);
    auto& buf = m_artBuffers[next];
    buf.count = std::min((uint32_t)maps.size(), 64u);
    for (uint32_t i = 0; i < buf.count; ++i) buf.maps[i] = maps[i];
    m_activeArtIdx.store(next, std::memory_order_release);
}

void MIDIOrchestrator::processBlock(MidiBuffer& buffer) {
    // 1. Handle MPE
    processMPE(buffer);

    // 2. Handle Articulation ID Translation in-place.  The event's
    // articulationId is the stable project-level mapping key; the MIDI
    // channel is the transport-level trigger selected by the user.
    const uint32_t artIdx = m_activeArtIdx.load(std::memory_order_acquire);
    const auto& artBuf = m_artBuffers[artIdx];
    
    if (artBuf.count > 0) {
        MidiEvent* events = const_cast<MidiEvent*>(buffer.getEvents());
        for (size_t i = 0; i < buffer.size(); ++i) {
            auto& ev = events[i];
            if (ev.size < 3 || ev.articulationId == 0) continue;
            for (uint32_t mapIndex = 0; mapIndex < artBuf.count; ++mapIndex) {
                const auto& map = artBuf.maps[mapIndex];
                if (map.id != ev.articulationId || map.triggerChannel == 0 ||
                    map.triggerChannel > 16) continue;
                const uint8_t status = ev.data[0] & 0xF0;
                if (status == 0x90 || status == 0x80) {
                    ev.data[0] = static_cast<uint8_t>(status | (map.triggerChannel - 1));
                }
                break;
            }
        }
    }
}

} // namespace Aura::Core::Engine
