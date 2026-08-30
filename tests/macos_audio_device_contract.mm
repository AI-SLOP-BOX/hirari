#if !defined(__APPLE__)
#error "This contract requires macOS CoreAudio"
#endif

#include <cassert>
#include <atomic>
#include <chrono>
#include <thread>

#include "../src/platform/audio_device.hpp"

namespace {
struct CallbackState {
    std::atomic<uint64_t> callbacks{0};
    std::atomic<uint64_t> frames{0};
};

void render(void* user, float** outputs, const float**, uint32_t frames) noexcept {
    auto* state = static_cast<CallbackState*>(user);
    if (state) {
        state->callbacks.fetch_add(1, std::memory_order_relaxed);
        state->frames.fetch_add(frames, std::memory_order_relaxed);
    }
    if (!outputs) return;
    for (uint32_t channel = 0; channel < 2; ++channel) {
        if (!outputs[channel]) continue;
        for (uint32_t frame = 0; frame < frames; ++frame) outputs[channel][frame] = 0.0f;
    }
}
}

int main() {
    auto device = Aura::Platform::createAudioDevice();
    assert(device);
    CallbackState callbackState;

    // Exercise the mono callback shape first. The CoreAudio backend must not
    // assume that every non-interleaved AudioBufferList has two buffers.
    const Aura::Platform::AudioDevice::Config first{48'000.0, 128, 0, 1};
    if (!device->initialize(first, &render, &callbackState)) {
        return 2;
    }
    assert(device->isHardwareAvailable());
    assert(!device->isSilentFallback());
    assert(device->config().sampleRate == first.sampleRate);
    assert(device->config().bufferSize == first.bufferSize);

    if (!device->start()) {
        device->stop();
        return 3;
    }
    const auto callbackDeadline = std::chrono::steady_clock::now() +
                                  std::chrono::seconds(2);
    while (callbackState.callbacks.load(std::memory_order_acquire) == 0 &&
           std::chrono::steady_clock::now() < callbackDeadline) {
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    assert(device->isRunning());
    if (callbackState.callbacks.load(std::memory_order_acquire) == 0 ||
        callbackState.frames.load(std::memory_order_acquire) == 0) {
        device->stop();
        return 4;
    }
    device->stop();
    assert(!device->isRunning());

    const Aura::Platform::AudioDevice::Config second{96'000.0, 512, 0, 2};
    assert(device->initialize(second, &render, &callbackState));
    assert(device->config().sampleRate == second.sampleRate);
    assert(device->config().bufferSize == second.bufferSize);
    assert(device->start());
    device->stop();
    return 0;
}
