// Real-device contract for the macOS audio path.
//
// `MacAudioDriverHost` is the adapter the engine actually constructs: the
// CoreAudio device lifecycle, format negotiation and realtime callback fan-out
// live in Rust (`hirari-core-bridge/src/mac_audio_driver.rs`) and are reached
// through `hirari_mac_audio_host_*`. This contract proves that the live path
// opens the real default output device, runs the callback, renegotiates a
// second format generation, and stops.
//
// The earlier form of this test drove `Hirari::Platform::createAudioDevice()`,
// whose only caller in the tree was this test itself, so it validated a
// translation unit the engine never used.
#if !defined(__APPLE__)
#error "This contract requires macOS CoreAudio"
#endif

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <thread>

#include "../src/core/driver/mac_audio_driver_host.hpp"

namespace {
using Hirari::Core::Driver::MacAudioDriverHost;

struct CallbackState {
    std::atomic<uint64_t> callbacks{0};
    std::atomic<uint64_t> frames{0};
};

void render(const float* const*, uint32_t, float* const* outputs,
            uint32_t outputChannels, uint32_t frames, void* user) noexcept {
    auto* state = static_cast<CallbackState*>(user);
    if (!state) return;
    state->callbacks.fetch_add(1, std::memory_order_relaxed);
    state->frames.fetch_add(frames, std::memory_order_relaxed);
    if (!outputs) return;
    for (uint32_t channel = 0; channel < outputChannels; ++channel) {
        if (!outputs[channel]) continue;
        for (uint32_t frame = 0; frame < frames; ++frame) outputs[channel][frame] = 0.0f;
    }
}
} // namespace

// Every rejection names itself: the script maps a non-zero exit to SKIPPED
// unless hardware is required, and a silent failure is indistinguishable from
// "no device on this host".
int fail(const char* reason, const MacAudioDriverHost& host, int code) {
    std::fprintf(stderr, "macOS CoreAudio contract: %s (status=%s, error=%d)\n", reason,
                 host.status(), host.last_error_code());
    return code;
}

int main() {
    MacAudioDriverHost host;
    CallbackState state;
    host.set_process_callback(&render, &state);

    // First generation: negotiate 48k/128 against the real device and start it.
    if (!host.reconfigure(48'000.0, 128)) {
        host.stop();
        return fail("cannot open the default output device at 48000/128", host, 2);
    }
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (state.callbacks.load(std::memory_order_acquire) == 0 &&
           std::chrono::steady_clock::now() < deadline) {
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    if (!host.is_running() ||
        state.callbacks.load(std::memory_order_acquire) == 0 ||
        state.frames.load(std::memory_order_acquire) == 0) {
        host.stop();
        return fail("the realtime callback never ran", host, 3);
    }
    if (host.sample_rate() != 48'000.0 || host.buffer_size() != 128 ||
        host.output_channel_count() == 0) {
        host.stop();
        return fail("the negotiated generation does not match the request", host, 4);
    }

    // Second generation: renegotiate while the driver is live.
    if (!host.reconfigure(96'000.0, 512)) {
        host.stop();
        return fail("cannot renegotiate the default output device to 96000/512", host, 5);
    }
    if (host.sample_rate() != 96'000.0 || host.buffer_size() != 512) {
        host.stop();
        return fail("the second generation does not match the request", host, 6);
    }
    if (!host.is_running()) {
        host.stop();
        return fail("the driver stopped after renegotiation", host, 7);
    }

    host.stop();
    if (host.is_running()) return fail("stop left the driver running", host, 8);
    return 0;
}
