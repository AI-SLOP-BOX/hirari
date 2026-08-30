#if defined(__APPLE__)
#import <AudioToolbox/AudioToolbox.h>
#import <AudioUnit/AudioUnit.h>

#include "../audio_device.hpp"
#include <algorithm>
#include <atomic>
#include <cstring>
#include <mutex>
#include <thread>

namespace Aura::Platform {

class CoreAudioDevice final : public AudioDevice {
public:
    ~CoreAudioDevice() override { stop(); }

    bool initialize(const Config& config, Callback callback, void* user) noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        stopUnlocked();
        if (!callback || !std::isfinite(config.sampleRate) || config.sampleRate <= 0.0 ||
            config.bufferSize == 0 || config.outputs == 0 || config.outputs > 2) {
            m_error = "invalid CoreAudio configuration";
            return false;
        }
        m_config = config;
        m_callback = callback;
        m_user = user;
        m_disconnected.store(false, std::memory_order_release);

        AudioComponentDescription desc{};
        desc.componentType = kAudioUnitType_Output;
        desc.componentSubType = kAudioUnitSubType_DefaultOutput;
        desc.componentManufacturer = kAudioUnitManufacturer_Apple;
        AudioComponent component = AudioComponentFindNext(nullptr, &desc);
        if (!component || AudioComponentInstanceNew(component, &m_unit) != noErr) {
            m_error = "CoreAudio default output component unavailable";
            return false;
        }

        AudioStreamBasicDescription format{};
        format.mSampleRate = config.sampleRate;
        format.mFormatID = kAudioFormatLinearPCM;
        format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
        format.mBytesPerPacket = sizeof(float);
        format.mFramesPerPacket = 1;
        format.mBytesPerFrame = sizeof(float);
        format.mChannelsPerFrame = std::min<uint32_t>(config.outputs, 2);
        format.mBitsPerChannel = 32;

        AURenderCallbackStruct callbackStruct{&renderCallback, this};
        if (AudioUnitSetProperty(m_unit, kAudioUnitProperty_SetRenderCallback,
                                 kAudioUnitScope_Input, 0, &callbackStruct, sizeof(callbackStruct)) != noErr ||
            AudioUnitSetProperty(m_unit, kAudioUnitProperty_StreamFormat,
                                 kAudioUnitScope_Input, 0, &format, sizeof(format)) != noErr ||
            AudioUnitInitialize(m_unit) != noErr) {
            stopUnlocked();
            m_error = "CoreAudio stream format or initialization failed";
            return false;
        }
        m_error.clear();
        return true;
    }

    bool start() noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        if (!m_unit) { m_error = "CoreAudio device is not initialized"; return false; }
        m_stopping.store(false, std::memory_order_release);
        m_running.store(true, std::memory_order_release);
        if (AudioOutputUnitStart(m_unit) != noErr) {
            stopUnlocked();
            m_error = "CoreAudio failed to start the output unit";
            return false;
        }
        return true;
    }

    void stop() noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        stopUnlocked();
    }

private:
    void stopUnlocked() noexcept {
        m_stopping.store(true, std::memory_order_release);
        const bool wasRunning = m_running.exchange(false, std::memory_order_acq_rel);

        if (m_unit) {
            // Stop delivery first.  AudioUnitUninitialize/Dispose must not run
            // while a callback can still be executing against this instance.
            if (wasRunning) AudioOutputUnitStop(m_unit);
            while (m_callbacksInFlight.load(std::memory_order_acquire) != 0) {
                std::this_thread::yield();
            }
            AudioUnitUninitialize(m_unit);
            AudioComponentInstanceDispose(m_unit);
            m_unit = nullptr;
        }
        m_callback = nullptr;
        m_user = nullptr;
        m_stopping.store(false, std::memory_order_release);
    }

    bool isRunning() const noexcept override { return m_running.load(std::memory_order_acquire); }
    const char* name() const noexcept override { return "CoreAudio Default Output"; }
    const char* lastError() const noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_error.c_str();
    }
    bool isHardwareAvailable() const noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_unit != nullptr && !m_disconnected.load(std::memory_order_acquire);
    }
    bool isSilentFallback() const noexcept override {
        return m_disconnected.load(std::memory_order_acquire);
    }
    Config config() const noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_config;
    }

    static OSStatus renderCallback(void* ref, AudioUnitRenderActionFlags* actionFlags, const AudioTimeStamp*,
                                   UInt32, UInt32 frames, AudioBufferList* data) {
        auto* self = static_cast<CoreAudioDevice*>(ref);
        if (!self) return noErr;

        self->m_callbacksInFlight.fetch_add(1, std::memory_order_acq_rel);
        struct CallbackGuard {
            CoreAudioDevice* device;
            ~CallbackGuard() {
                device->m_callbacksInFlight.fetch_sub(1, std::memory_order_release);
            }
        } guard{self};

        if (self->m_stopping.load(std::memory_order_acquire) ||
            !self->m_running.load(std::memory_order_acquire)) {
            silence(data, frames);
            return noErr;
        }

        if (actionFlags && (*actionFlags & kAudioUnitRenderAction_PostRenderError) != 0) {
            self->m_disconnected.store(true, std::memory_order_release);
            silence(data, frames);
            return noErr;
        }

        const uint32_t expectedChannels = std::min<uint32_t>(self->m_config.outputs, 2);
        if (!data || expectedChannels == 0 || data->mNumberBuffers < expectedChannels ||
            !self->m_callback) {
            self->m_disconnected.store(true, std::memory_order_release);
            silence(data, frames);
            return noErr;
        }

        for (uint32_t channel = 0; channel < expectedChannels; ++channel) {
            if (!data->mBuffers[channel].mData) {
                self->m_disconnected.store(true, std::memory_order_release);
                silence(data, frames);
                return noErr;
            }
        }

        if (self->m_disconnected.load(std::memory_order_acquire)) {
            silence(data, frames);
            return noErr;
        }

        float* outputs[2] = {static_cast<float*>(data->mBuffers[0].mData), nullptr};
        if (expectedChannels > 1) {
            outputs[1] = static_cast<float*>(data->mBuffers[1].mData);
        }
        self->m_callback(self->m_user, outputs, nullptr, frames);
        return noErr;
    }

    static void silence(AudioBufferList* data, UInt32 frames) noexcept {
        if (!data) return;
        for (UInt32 i = 0; i < data->mNumberBuffers; ++i) {
            auto& buffer = data->mBuffers[i];
            if (!buffer.mData) continue;
            const UInt32 available = buffer.mDataByteSize / sizeof(float);
            const UInt32 count = std::min(frames, available);
            std::memset(buffer.mData, 0, static_cast<size_t>(count) * sizeof(float));
        }
    }

    Config m_config{};
    Callback m_callback = nullptr;
    void* m_user = nullptr;
    AudioUnit m_unit = nullptr;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stopping{false};
    std::atomic<bool> m_disconnected{false};
    std::atomic<uint32_t> m_callbacksInFlight{0};
    mutable std::mutex m_lifecycleMutex;
    std::string m_error;
};

std::unique_ptr<AudioDevice> createAudioDevice() {
    return std::make_unique<CoreAudioDevice>();
}

} // namespace Aura::Platform
#endif
