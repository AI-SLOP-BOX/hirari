#include <cassert>
#include "../src/core/audio_buffer.hpp"

int main() {
    using Hirari::Core::AudioBuffer;
    AudioBuffer::resetRealtimeResizeStats();
    AudioBuffer buffer(2, 16);
    AudioBuffer::setRTThread(true);
    assert(!buffer.resize(2, 128));
    AudioBuffer::setRTThread(false);
    assert(AudioBuffer::realtimeResizeAttempts() == 1);
    assert(AudioBuffer::lastRealtimeRequestedCapacity() >= 128);

    // The AutomationManager block that lived here moved to
    // `automation_manager::tests::assertions_moved_from_the_cpp_contract_tests_hold_without_the_facade`.
    // `src/core/engine/automation_manager.hpp` was a forwarding facade over the
    // Rust-owned `hirari_automation_manager_*` symbols and no production code
    // included it; each assertion it made is now observed directly in Rust.

    return 0;
}
