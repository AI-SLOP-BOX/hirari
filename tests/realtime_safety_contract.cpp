#include <cassert>

#include "../src/core/audio_buffer.hpp"
#include "../src/core/rust_ffi.hpp"
#include "../src/core/status_queue.hpp"

int main() {
    using namespace Hirari::Core;

    // Drain notifications left by other contract tests in the same process.
    StatusMessage message{};
    while (popEngineStatus(message)) {}

    // The Rust event table is the owner; the contract drives it through the
    // same FFI the engine's block head uses.
    void* midi = hirari_midi_buffer_create();
    for (size_t i = 0; i < kHirariMidiMaxEventsPerBlock; ++i) {
        hirari_midi_buffer_add_note_on(midi, 1, 60, 100, i, 0);
    }
    hirari_midi_buffer_add_note_on(midi, 1, 61, 100, kHirariMidiMaxEventsPerBlock, 0);
    assert(hirari_midi_buffer_event_count(midi) == kHirariMidiMaxEventsPerBlock);
    assert(hirari_midi_buffer_overflowed(midi));
    assert(hirari_midi_buffer_dropped_count(midi) == 1);
    assert(hirari_midi_buffer_take_overflowed(midi));
    assert(hirari_midi_buffer_take_dropped(midi) == 1);
    assert(!hirari_midi_buffer_overflowed(midi));

    void* emptyPayload = hirari_midi_buffer_create();
    HirariMidiEvent emptyEvent{};
    emptyEvent.size = 0;
    assert(hirari_midi_buffer_copy(emptyPayload, &emptyEvent));
    assert(hirari_midi_buffer_event_count(emptyPayload) == 1);
    const auto* emptyEvents =
        static_cast<const HirariMidiEvent*>(hirari_midi_buffer_event_data(emptyPayload));
    assert(emptyEvents[0].data[0] == 0);
    hirari_midi_buffer_destroy(midi);
    hirari_midi_buffer_destroy(emptyPayload);

    AudioBuffer buffer(1, 16);
    const auto attemptsBefore = AudioBuffer::realtimeResizeAttempts();
    AudioBuffer::setRTThread(true);
    buffer.reserve(2, 64);
    AudioBuffer::setRTThread(false);

    assert(AudioBuffer::realtimeResizeAttempts() == attemptsBefore + 1);
    assert(popEngineStatus(message));
    assert(message.severity == StatusSeverity::Critical);

    return 0;
}
