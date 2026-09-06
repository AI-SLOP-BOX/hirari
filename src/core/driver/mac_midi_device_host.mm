#include "mac_midi_device_host.hpp"

#if defined(__APPLE__)
#include <CoreMIDI/CoreMIDI.h>
#include <sstream>
#include <deque>
#include <mutex>

namespace Aura::Core::Driver {

namespace {
struct MidiInputEvent { MIDIUniqueID source = 0; std::vector<uint8_t> data; };
std::mutex inputMutex;
std::deque<MidiInputEvent> inputQueue;
MIDIClientRef inputClient = 0;
MIDIPortRef inputPort = 0;

void readInput(const MIDIPacketList* list, void* /*readProcRefCon*/, void* srcConnRefCon) {
    if (!list) return;
    MIDIUniqueID sourceId = 0;
    if (srcConnRefCon) {
        MIDIObjectGetIntegerProperty(static_cast<MIDIEndpointRef>(
                reinterpret_cast<uintptr_t>(srcConnRefCon)),
            kMIDIPropertyUniqueID, &sourceId);
    }
    std::lock_guard<std::mutex> lock(inputMutex);
    const MIDIPacket* packet = &list->packet[0];
    for (UInt32 i = 0; i < list->numPackets; ++i) {
        if (!packet) break;
        if (packet->length > 0 && packet->length <= 256 && inputQueue.size() < 1024)
            inputQueue.push_back(MidiInputEvent{sourceId, std::vector<uint8_t>(packet->data, packet->data + packet->length)});
        packet = MIDIPacketNext(packet);
    }
}
std::string endpointName(MIDIEndpointRef endpoint) {
    CFStringRef value = nullptr;
    if (MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &value) != noErr || !value) return {};
    char buffer[512] = {};
    const bool converted = CFStringGetCString(value, buffer, sizeof(buffer), kCFStringEncodingUTF8);
    CFRelease(value);
    return converted ? std::string(buffer) : std::string{};
}
}

std::string list_core_midi_devices_json() {
    std::ostringstream json;
    json << '[';
    bool first = true;
    auto append = [&](MIDIEndpointRef endpoint, const char* direction, ItemCount index) {
        const std::string name = endpointName(endpoint);
        if (name.empty()) return;
        MIDIUniqueID uniqueId = 0;
        MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &uniqueId);
        if (!first) json << ',';
        first = false;
        json << "{\"id\":\"coremidi:" << uniqueId << ":" << direction
             << "\",\"name\":\"";
        for (const char c : name) {
            if (c == '\\' || c == '"') json << '\\';
            json << c;
        }
        json << "\",\"direction\":\"" << direction << "\",\"index\":" << index << '}';
    };
    for (ItemCount i = 0; i < MIDIGetNumberOfSources(); ++i) append(MIDIGetSource(i), "input", i);
    for (ItemCount i = 0; i < MIDIGetNumberOfDestinations(); ++i) append(MIDIGetDestination(i), "output", i);
    json << ']';
    return json.str();
}

bool send_core_midi_message(uint32_t uniqueId, const uint8_t* data, size_t size) {
    if (!data || size == 0 || size > 256) return false;
    MIDIEndpointRef destination = 0;
    for (ItemCount i = 0; i < MIDIGetNumberOfDestinations(); ++i) {
        const MIDIEndpointRef candidate = MIDIGetDestination(i);
        MIDIUniqueID candidateId = 0;
        if (candidate && MIDIObjectGetIntegerProperty(candidate, kMIDIPropertyUniqueID, &candidateId) == noErr &&
            static_cast<uint32_t>(candidateId) == uniqueId) {
            destination = candidate;
            break;
        }
    }
    if (!destination) return false;
    MIDIClientRef client = 0;
    MIDIPortRef port = 0;
    if (MIDIClientCreate(CFSTR("Aura MIDI"), nullptr, nullptr, &client) != noErr ||
        MIDIOutputPortCreate(client, CFSTR("Aura Output"), &port) != noErr) {
        if (client) MIDIClientDispose(client);
        return false;
    }
    uint8_t packetStorage[sizeof(MIDIPacketList) + 256] = {};
    auto* packets = reinterpret_cast<MIDIPacketList*>(packetStorage);
    MIDIPacket* packet = MIDIPacketListInit(packets);
    packet = MIDIPacketListAdd(packets, sizeof(packetStorage), packet, 0, size, data);
    const OSStatus status = packet ? MIDISend(port, destination, packets) : -1;
    MIDIPortDispose(port);
    MIDIClientDispose(client);
    return status == noErr;
}

bool start_core_midi_input() {
    std::lock_guard<std::mutex> lock(inputMutex);
    if (inputPort) return true;
    if (MIDIClientCreate(CFSTR("Aura MIDI Input"), nullptr, nullptr, &inputClient) != noErr ||
        MIDIInputPortCreate(inputClient, CFSTR("Aura Input"), readInput, nullptr, &inputPort) != noErr) {
        if (inputPort) MIDIPortDispose(inputPort);
        if (inputClient) MIDIClientDispose(inputClient);
        inputPort = 0; inputClient = 0; return false;
    }
    for (ItemCount i = 0; i < MIDIGetNumberOfSources(); ++i) {
        auto source = MIDIGetSource(i);
        if (MIDIPortConnectSource(inputPort, source,
                reinterpret_cast<void*>(source)) != noErr) continue;
    }
    return true;
}

void stop_core_midi_input() {
    std::lock_guard<std::mutex> lock(inputMutex);
    if (inputPort) MIDIPortDispose(inputPort);
    if (inputClient) MIDIClientDispose(inputClient);
    inputPort = 0; inputClient = 0; inputQueue.clear();
}

std::string poll_core_midi_input_json() {
    std::lock_guard<std::mutex> lock(inputMutex);
    std::ostringstream json; json << '[';
    bool first = true;
    while (!inputQueue.empty()) {
        auto event = std::move(inputQueue.front()); inputQueue.pop_front();
        if (!first) json << ','; first = false;
        json << "{\"source_unique_id\":" << event.source << ",\"data_hex\":\"";
        static constexpr char hex[] = "0123456789abcdef";
        for (uint8_t byte : event.data) json << hex[byte >> 4] << hex[byte & 0xf];
        json << "\"}";
    }
    json << ']'; return json.str();
}

} // namespace Aura::Core::Driver
#endif
