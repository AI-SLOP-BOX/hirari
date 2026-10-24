// Frozen fixed-buffer writes from the former C++ MidiBuffer implementation.
#include <algorithm>
#include <cstddef>
#include <cstdint>

namespace {
struct MidiEventReference {
    uint64_t sample_offset;
    uint32_t size;
    uint8_t data[256];
    uint8_t articulation_id;
};
static_assert(sizeof(MidiEventReference) == 272);
}

extern "C" uint8_t midi_buffer_reference_add_event(
    void* storage, size_t capacity, size_t* count, uint64_t sample_offset,
    const uint8_t* data, uint32_t size, uint8_t articulation_id) {
    if (size > 256) return 1;
    if (size != 0 && data == nullptr) return 2;
    if (!storage || !count) return 4;
    if (*count >= capacity) return 3;
    auto* events = static_cast<MidiEventReference*>(storage);
    auto& event = events[(*count)++];
    event.sample_offset = sample_offset;
    event.size = size;
    event.articulation_id = articulation_id;
    if (size > 0) std::copy(data, data + size, event.data);
    std::fill(event.data + size, event.data + 256, uint8_t{0});
    return 0;
}

extern "C" uint8_t midi_buffer_reference_copy_event(
    void* storage, size_t capacity, size_t* count, const void* source) {
    if (!source) return 4;
    const auto event = *static_cast<const MidiEventReference*>(source);
    if (event.size > 256) return 1;
    if (!storage || !count) return 4;
    if (*count >= capacity) return 3;
    auto* events = static_cast<MidiEventReference*>(storage);
    events[(*count)++] = event;
    std::fill(events[*count - 1].data + event.size,
              events[*count - 1].data + 256, uint8_t{0});
    return 0;
}
