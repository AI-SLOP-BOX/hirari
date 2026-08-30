#include <cassert>

#include "../src/core/audio_buffer.hpp"
#include "../src/core/midi_buffer.hpp"
#include "../src/core/status_queue.hpp"
#include "../src/platform/audio_device.hpp"

int main() {
    using namespace Aura::Core;

    // Drain notifications left by other contract tests in the same process.
    StatusQueue::Message message{};
    while (StatusQueue::getInstance().pop(message)) {}

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
    Aura::Core::MidiEvent emptyEvent{};
    emptyEvent.size = 0;
    assert(emptyPayload.tryAddEvent(emptyEvent));
    assert(emptyPayload.size() == 1);
    assert(emptyPayload.getEvents()[0].data[0] == 0);

    Aura::Platform::SilentAudioDevice device;
    Aura::Platform::AudioDevice::Config config;
    config.sampleRate = 48000.0;
    config.bufferSize = 128;
    assert(device.initialize(config, nullptr, nullptr));
    assert(device.config().sampleRate == 48000.0);
    assert(!device.start());
    assert(device.hasError());
    assert(device.isSilentFallback());

    AudioBuffer buffer(1, 16);
    const auto attemptsBefore = AudioBuffer::realtimeResizeAttempts();
    AudioBuffer::setRTThread(true);
    buffer.reserve(2, 64);
    AudioBuffer::setRTThread(false);

    assert(AudioBuffer::realtimeResizeAttempts() == attemptsBefore + 1);
    assert(StatusQueue::getInstance().pop(message));
    assert(message.severity == StatusQueue::Severity::Critical);

    return 0;
}
