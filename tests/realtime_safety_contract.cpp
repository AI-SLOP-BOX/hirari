#include <cassert>

#include "../src/core/audio_buffer.hpp"
#include "../src/core/midi_buffer.hpp"
#include "../src/core/status_queue.hpp"

int main() {
    using namespace Hirari::Core;

    // Drain notifications left by other contract tests in the same process.
    StatusMessage message{};
    while (popEngineStatus(message)) {}

    MidiBuffer midi;
    for (size_t i = 0; i < MidiBuffer::kMaxEventsPerBlock; ++i) {
        midi.addNoteOn(1, 60, 100, i);
    }
    midi.addNoteOn(1, 61, 100, MidiBuffer::kMaxEventsPerBlock);
    assert(midi.size() == MidiBuffer::kMaxEventsPerBlock);
    assert(midi.overflowed());
    assert(midi.droppedEvents() == 1);
    assert(midi.takeOverflowed());
    assert(midi.takeDroppedEvents() == 1);
    assert(!midi.overflowed());

    MidiBuffer emptyPayload;
    Hirari::Core::MidiEvent emptyEvent{};
    emptyEvent.size = 0;
    assert(emptyPayload.tryAddEvent(emptyEvent));
    assert(emptyPayload.size() == 1);
    assert(emptyPayload.getEvents()[0].data[0] == 0);

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
