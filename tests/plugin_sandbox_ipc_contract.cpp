#include <cassert>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <thread>
#include <fcntl.h>
#include <unistd.h>

#include "../src/core/plugins/plugin_sandbox_host.hpp"

int main(int argc, char** argv) {
#if defined(_WIN32)
    (void)argc; (void)argv;
    return 0;
#else
    assert(argc >= 2);
    ::setenv("AURA_PLUGIN_HOST_BIN", argv[1], 1);
    // Make the in-flight mailbox window deterministic. The worker only
    // honors this test hook when explicit fault injection is enabled.
    ::setenv("AURA_PLUGIN_TEST_FAULTS", "1", 1);
    ::setenv("AURA_PLUGIN_WORKER_DELAY_MS", "50", 1);

    // An optional CLAP fixture enables the MIDI echo assertion. The builtin
    // passthrough remains the portable audio-only fallback for environments
    // that do not build fixtures.
    const bool hasMidiFixture = argc >= 3;
    Aura::Core::Plugins::PluginSandboxHost host(
        hasMidiFixture ? argv[2] : "builtin://passthrough");
    assert(host.start());
    assert(host.isAlive());
    assert(host.failure() == Aura::Core::Plugins::PluginSandboxHost::Failure::None);
    // Starting an already-live host is an idempotency error, not an invalid
    // path.  Preserve the healthy diagnostic state for UI/recovery callers.
    assert(!host.start());
    assert(host.isAlive());
    assert(host.failure() == Aura::Core::Plugins::PluginSandboxHost::Failure::None);
    assert(host.activeSampleRate() == 44100);
    assert(host.activeChannels() == 2);

    Aura::Core::AudioBuffer buffer(2, 32);
    for (uint32_t channel = 0; channel < 2; ++channel) {
        float* samples = buffer.getWritePointer(channel);
        for (uint32_t frame = 0; frame < 32; ++frame) samples[frame] = 0.1f * static_cast<float>(frame + channel);
    }
    Aura::Core::MidiBuffer midi;
    const uint8_t noteOn[] = {0x90, 60, 100};
    if (hasMidiFixture) midi.addEvent(7, noteOn, sizeof(noteOn));
    std::array<uint8_t, 300> extendedMidi{};
    for (size_t index = 0; index < extendedMidi.size(); ++index)
        extendedMidi[index] = static_cast<uint8_t>(index & 0x7f);
    if (hasMidiFixture) {
        assert(host.enqueueExtendedMidi(11, 0, extendedMidi.data(), extendedMidi.size()));
    }
    const bool first = host.process(buffer, midi);
    assert(!first); // The helper completes asynchronously; audio must not block.

    // Submit while the previous block is still in flight.  The realtime path
    // must remain non-blocking and expose the bounded fallback explicitly.
    for (int attempt = 0; attempt < 4; ++attempt) {
        Aura::Core::MidiBuffer pollMidi;
        (void)host.process(buffer, pollMidi);
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    assert(host.takeMailboxOverruns() > 0);

    bool completed = false;
    Aura::Core::MidiBuffer completedMidi;
    const auto completionDeadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(250);
    while (std::chrono::steady_clock::now() < completionDeadline) {
        std::this_thread::yield();
        if (host.process(buffer, completedMidi)) { completed = true; break; }
    }
    assert(completed);
    if (hasMidiFixture) {
        assert(completedMidi.size() == 1);
        assert(completedMidi.getEvents()[0].sampleOffset == 7);
        assert(completedMidi.getEvents()[0].size == sizeof(noteOn));
        assert(completedMidi.getEvents()[0].data[0] == noteOn[0]);
        assert(std::fabs(buffer.getReadPointer(0)[1] - 0.025f) < 0.001f);
    } else {
        assert(completedMidi.size() == 0);
    }
    assert(host.isAlive());
    assert(host.setGenerationContext({1, 1, 1, 1}));
    const uint8_t unsupportedState[] = {0x00, 0x00, 0x80, 0x7f}; // +inf
    assert(!host.setState(unsupportedState, sizeof(unsupportedState)));
    assert(host.restart());
    assert(host.isAlive());
    host.stop();
    assert(!host.isAlive());

    // The channel count is part of the worker activation contract. Verify
    // that a mono host does not silently fall back to the stereo layout.
    Aura::Core::Plugins::PluginSandboxHost mono("builtin://passthrough", 48000.0, 64, 1);
    assert(mono.start());
    assert(mono.activeSampleRate() == 48000);
    assert(mono.activeChannels() == 1);
    Aura::Core::AudioBuffer monoBuffer(1, 16);
    for (uint32_t frame = 0; frame < 16; ++frame)
        monoBuffer.getWritePointer(0)[frame] = 0.25f * static_cast<float>(frame);
    Aura::Core::MidiBuffer monoMidi;
    assert(!mono.process(monoBuffer, monoMidi));
    bool monoCompleted = false;
    const auto monoDeadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(250);
    while (std::chrono::steady_clock::now() < monoDeadline) {
        std::this_thread::yield();
        if (mono.process(monoBuffer, monoMidi)) { monoCompleted = true; break; }
    }
    assert(monoCompleted);
    mono.stop();

    // A wider caller buffer must be rejected rather than silently truncating
    // the route to the worker's fixed mailbox layout. Silent truncation is
    // especially dangerous for surround/sidechain graphs because the result
    // still looks like valid audio while channels are missing.
    Aura::Core::Plugins::PluginSandboxHost stereoOnly("builtin://passthrough");
    assert(stereoOnly.start());
    Aura::Core::AudioBuffer wideBuffer(3, 16);
    for (uint32_t channel = 0; channel < 3; ++channel)
        for (uint32_t frame = 0; frame < 16; ++frame)
            wideBuffer.getWritePointer(channel)[frame] = 1.0f;
    Aura::Core::MidiBuffer wideMidi;
    wideMidi.addEvent(0, noteOn, sizeof(noteOn));
    assert(!stereoOnly.process(wideBuffer, wideMidi));
    for (uint32_t channel = 0; channel < 3; ++channel)
        for (uint32_t frame = 0; frame < 16; ++frame)
            assert(wideBuffer.getReadPointer(channel)[frame] == 0.0f);
    assert(wideMidi.size() == 0);
    stereoOnly.stop();

    const char* unsupportedPath = "/tmp/aura_sandbox_test.vst3";
    const int unsupportedFd = ::open(unsupportedPath, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    assert(unsupportedFd >= 0);
    const char marker = 0;
    assert(::write(unsupportedFd, &marker, 1) == 1);
    ::close(unsupportedFd);
    Aura::Core::Plugins::PluginSandboxHost unsupported(unsupportedPath);
    assert(!unsupported.start());
    assert(unsupported.failure() == Aura::Core::Plugins::PluginSandboxHost::Failure::PluginFormatUnsupported);
    ::unlink(unsupportedPath);

    const char* symlinkTargetPath = "/tmp/aura_sandbox_symlink_target.clap";
    const char* symlinkPath = "/tmp/aura_sandbox_symlink.clap";
    const int targetFd = ::open(symlinkTargetPath, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    assert(targetFd >= 0);
    assert(::write(targetFd, &marker, 1) == 1);
    ::close(targetFd);
    ::unlink(symlinkPath);
    assert(::symlink(symlinkTargetPath, symlinkPath) == 0);
    Aura::Core::Plugins::PluginSandboxHost symlinked(symlinkPath);
    assert(!symlinked.start());
    assert(symlinked.failure() == Aura::Core::Plugins::PluginSandboxHost::Failure::InvalidPluginPath);
    ::unlink(symlinkPath);
    ::unlink(symlinkTargetPath);
    return 0;
#endif
}
