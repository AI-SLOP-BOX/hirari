#pragma once
#if defined(__APPLE__)
#include <AudioUnit/AudioUnit.h>
#include <CoreAudio/CoreAudio.h>
#endif
#include <functional>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <utility>

#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
#include <xmmintrin.h>
#endif

namespace Aura::Core::Driver {

/**
 * @class MacAudioDriver
 * @brief Professional Low-Latency CoreAudio (HAL) Driver for macOS.
 * Uses the native AudioDeviceID and AudioHardwarePropertyListener.
 */
class MacAudioDriver {
public:
    using ProcessCallback = std::function<void(float* l, float* r, uint32_t len)>;

    // The sink and its context are owned by the registrant and must remain
    // alive until unregister_input_capture_sink() returns. The callback runs
    // on CoreAudio's realtime thread: it must not allocate, lock, call into
    // Rust/FFI, or retain the supplied channel pointers. The channel memory
    // is owned by this driver and is valid only for the duration of the call.
    using InputCaptureCallback = void (*)(void* context,
                                          const float* const* channels,
                                          uint32_t channelCount,
                                          uint32_t frameCount,
                                          const AudioTimeStamp* timestamp) noexcept;

    struct InputCaptureSink {
        InputCaptureCallback callback = nullptr;
        void* context = nullptr;
    };

    MacAudioDriver(ProcessCallback callback) : m_unit(nullptr), m_callback(std::move(callback)) {}
    ~MacAudioDriver() { stop(); }

    bool start(double sampleRate, uint32_t bufferSize, AudioDeviceID requestedDevice = kAudioObjectUnknown) {
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        // Reject an invalid device generation before touching CoreAudio.  A
        // device reconfiguration can briefly publish zero/NaN values; trying
        // to initialize an AudioUnit with them leaves teardown and recovery
        // state indistinguishable from a real device failure.
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || sampleRate > 384000.0 ||
            bufferSize == 0 || bufferSize > kMaxFramesPerSlice) {
            return false;
        }
        const auto* registeredInputSink = m_inputSink.load(std::memory_order_acquire);
        stopUnlocked();
        // start() is also the restart path. Preserve an explicitly registered
        // sink across teardown; stop() itself still disconnects it.
        if (registeredInputSink)
            m_inputSink.store(registeredInputSink, std::memory_order_release);
        m_sampleRate = sampleRate;
        m_bufferSize = bufferSize;

        AudioComponentDescription desc = {
            kAudioUnitType_Output,
            kAudioUnitSubType_HALOutput, // INDUSTRIAL: Direct hardware control
            kAudioUnitManufacturer_Apple,
            0, 0
        };

        AudioComponent comp = AudioComponentFindNext(NULL, &desc);
        if (!comp) return failStart(-1);

        OSStatus err = AudioComponentInstanceNew(comp, &m_unit);
        if (err != noErr) return failStart(err);
        if (requestedDevice != kAudioObjectUnknown) {
            UInt32 deviceSize = sizeof(requestedDevice);
            err = AudioUnitSetProperty(m_unit, kAudioOutputUnitProperty_CurrentDevice,
                                       kAudioUnitScope_Global, 0, &requestedDevice, deviceSize);
            if (err != noErr) return failStart(err);
        }

        // AUHAL starts with both directions disabled.  Bus 0 is the device
        // output: the client supplies samples on its Input scope.  Bus 1 is
        // the device input: the client receives samples on its Output scope.
        // The EnableIO property, however, is addressed on the *device-side*
        // scope/element pair.  Using the opposite pair makes AudioUnit
        // initialization fail on real CoreAudio devices even though the
        // stream-format calls look valid.
        UInt32 enable = 1;
        err = AudioUnitSetProperty(m_unit, kAudioOutputUnitProperty_EnableIO,
                                   kAudioUnitScope_Output, 0, &enable, sizeof(enable));
        if (err != noErr) return failStart(err);
        err = AudioUnitSetProperty(m_unit, kAudioOutputUnitProperty_EnableIO,
                                   kAudioUnitScope_Input, 1, &enable, sizeof(enable));
        if (err != noErr) return failStart(err);

        // --- 1. SET STREAM FORMAT (32-bit Float Non-Interleaved) ---
        AudioStreamBasicDescription format{};
        format.mSampleRate = sampleRate;
        format.mFormatID = kAudioFormatLinearPCM;
        format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
        format.mBitsPerChannel = 32;
        format.mChannelsPerFrame = 2;
        format.mFramesPerPacket = 1;
        format.mBytesPerPacket = 4;
        format.mBytesPerFrame = 4;

        err = AudioUnitSetProperty(m_unit, kAudioUnitProperty_StreamFormat,
                                   kAudioUnitScope_Input, 0, &format, sizeof(format));
        if (err != noErr) return failStart(err);
        err = AudioUnitSetProperty(m_unit, kAudioUnitProperty_StreamFormat,
                                   kAudioUnitScope_Output, 1, &format, sizeof(format));
        if (err != noErr) return failStart(err);

        m_inputChannelCount = std::min<uint32_t>(format.mChannelsPerFrame, kMaxInputChannels);
        if (m_inputChannelCount == 0) return failStart();
        const size_t inputSampleCount = static_cast<size_t>(m_inputChannelCount) * bufferSize;
        std::unique_ptr<float[]> inputBuffer(new (std::nothrow) float[inputSampleCount]);
        if (!inputBuffer) return failStart();
        m_inputBuffer = std::move(inputBuffer);
        m_inputBufferFrames = bufferSize;

        // --- 2. SET BUFFER SIZE ---
        err = AudioUnitSetProperty(m_unit, kAudioUnitProperty_MaximumFramesPerSlice,
                                   kAudioUnitScope_Global, 0, &bufferSize, sizeof(bufferSize));
        if (err != noErr) return failStart(err);

        // --- 3. SET RENDER CALLBACK ---
        AURenderCallbackStruct cb;
        cb.inputProc = [](void* ref, AudioUnitRenderActionFlags* flags, const AudioTimeStamp* time,
                          UInt32, UInt32 frames, AudioBufferList* data) -> OSStatus {
            // INDUSTRIAL: Silicon Safety (DAZ/FTZ)
#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
            struct MxcsrGuard {
                unsigned int value = _mm_getcsr();
                MxcsrGuard() { _mm_setcsr(value | 0x8040); }
                ~MxcsrGuard() { _mm_setcsr(value); }
            } mxcsrGuard;
#endif

            auto* self = static_cast<MacAudioDriver*>(ref);
            if (!self) return noErr;
            self->m_callbacksInFlight.fetch_add(1, std::memory_order_acquire);
            struct CallbackGuard {
                MacAudioDriver* driver;
                ~CallbackGuard() {
                    driver->m_callbacksInFlight.fetch_sub(1, std::memory_order_release);
                }
            } guard{self};

            if (!data || data->mNumberBuffers == 0) return noErr;

            // Pull hardware input into storage allocated during start(). The
            // AudioBufferList is stack-only and contains no ownership.
            const auto* inputSink = self->m_inputSink.load(std::memory_order_acquire);
            if (!self->m_stopping.load(std::memory_order_acquire) &&
                self->m_running.load(std::memory_order_acquire) &&
                inputSink && inputSink->callback &&
                frames <= self->m_inputBufferFrames && self->m_inputBuffer) {
                struct PlanarInputBufferList {
                    UInt32 mNumberBuffers;
                ::AudioBuffer mBuffers[kMaxInputChannels];
                } inputData{};
                inputData.mNumberBuffers = self->m_inputChannelCount;
                for (UInt32 channel = 0; channel < self->m_inputChannelCount; ++channel) {
                    inputData.mBuffers[channel].mNumberChannels = 1;
                    inputData.mBuffers[channel].mDataByteSize = frames * sizeof(float);
                    inputData.mBuffers[channel].mData =
                        self->m_inputBuffer.get() + static_cast<size_t>(channel) * self->m_inputBufferFrames;
                }
                if (AudioUnitRender(self->m_unit, flags, time, 1, frames,
                                    reinterpret_cast<AudioBufferList*>(&inputData)) == noErr) {
                    const float* channels[kMaxInputChannels] = {};
                    for (UInt32 channel = 0; channel < self->m_inputChannelCount; ++channel)
                        channels[channel] = static_cast<const float*>(inputData.mBuffers[channel].mData);
                    inputSink->callback(inputSink->context, channels,
                                        self->m_inputChannelCount, frames, time);
                }
            }

            // The configured format is non-interleaved, but some devices can
            // renegotiate a single interleaved buffer.  Handle both layouts
            // and always provide silence while the driver is stopping.
            if (self->m_stopping.load(std::memory_order_acquire) ||
                !self->m_running.load(std::memory_order_acquire) || !self->m_callback) {
                for (UInt32 b = 0; b < data->mNumberBuffers; ++b) {
                    if (data->mBuffers[b].mData && data->mBuffers[b].mNumberChannels > 0)
                        std::fill_n(static_cast<float*>(data->mBuffers[b].mData), frames * data->mBuffers[b].mNumberChannels, 0.0f);
                }
            } else if (frames > self->m_bufferSize || frames > kMaxFramesPerSlice) {
                // CoreAudio may briefly renegotiate the block size during a
                // device switch. Never pass a larger block into an engine
                // prepared for the previous generation; emit bounded silence
                // until the control plane completes reconfiguration.
                for (UInt32 b = 0; b < data->mNumberBuffers; ++b) {
                    if (data->mBuffers[b].mData && data->mBuffers[b].mNumberChannels > 0)
                        std::fill_n(static_cast<float*>(data->mBuffers[b].mData),
                                    frames * data->mBuffers[b].mNumberChannels, 0.0f);
                }
            } else if (data->mNumberBuffers >= 2) {
                auto* outL = static_cast<float*>(data->mBuffers[0].mData);
                auto* outR = static_cast<float*>(data->mBuffers[1].mData);
                if (outL && outR) self->m_callback(outL, outR, frames);
            } else if (data->mBuffers[0].mData) {
                // The engine callback operates on separate channel spans. Do
                // not pretend an interleaved buffer has that layout; silence
                // is safer than corrupting the device buffer.
                std::fill_n(static_cast<float*>(data->mBuffers[0].mData),
                            frames * std::max<UInt32>(1, data->mBuffers[0].mNumberChannels), 0.0f);
            }

            return noErr;
        };
        cb.inputProcRefCon = this;
        err = AudioUnitSetProperty(m_unit, kAudioUnitProperty_SetRenderCallback,
                                   kAudioUnitScope_Input, 0, &cb, sizeof(cb));
        if (err != noErr) return failStart(err);

        err = AudioUnitInitialize(m_unit);
        if (err != noErr) return failStart(err);
        m_initialized = true;

        AudioDeviceID device = kAudioObjectUnknown;
        UInt32 size = sizeof(device);
        if (AudioUnitGetProperty(m_unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &device, &size) == noErr &&
            device != kAudioObjectUnknown) {
            m_device = device;
            AudioObjectPropertyAddress address = { kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
            if (AudioObjectAddPropertyListener(m_device, &address, deviceListener, this) == noErr)
                m_deviceListenerInstalled = true;
        }

        m_deviceLost.store(false, std::memory_order_release);
        m_running.store(true, std::memory_order_release);
        err = AudioOutputUnitStart(m_unit);
        if (err != noErr) {
            m_running.store(false, std::memory_order_release);
            return failStart(err);
        }
        m_lastErrorCode.store(0, std::memory_order_release);
        return true;
    }

    void stop() {
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        stopUnlocked();
    }

    void stopUnlocked() {
        m_stopping.store(true, std::memory_order_release);
        const bool wasRunning = m_running.exchange(false, std::memory_order_acq_rel);
        m_inputSink.exchange(nullptr, std::memory_order_acq_rel);
        wait_for_callbacks_to_quiesce();
        if (m_deviceListenerInstalled) {
            AudioObjectPropertyAddress address = { kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
            AudioObjectRemovePropertyListener(m_device, &address, deviceListener, this);
            m_deviceListenerInstalled = false;
        }
        if (m_unit) {
            if (wasRunning) {
                AudioOutputUnitStop(m_unit);
            }
            // AudioOutputUnitStop prevents new callbacks, but a callback that
            // was already admitted may still be using this object.  Reclaim
            // the AudioUnit only after that callback has left.
            while (m_callbacksInFlight.load(std::memory_order_acquire) != 0) {
                std::this_thread::yield();
            }
            if (m_initialized) AudioUnitUninitialize(m_unit);
            AudioComponentInstanceDispose(m_unit);
            m_unit = nullptr;
            m_initialized = false;
        }
        m_inputBuffer.reset();
        m_inputBufferFrames = 0;
        m_inputChannelCount = 0;
        m_stopping.store(false, std::memory_order_release);
    }

    bool is_running() const { return m_running.load(std::memory_order_acquire); }
    bool device_lost() const { return m_deviceLost.load(std::memory_order_acquire); }
    int32_t last_error_code() const noexcept {
        return m_lastErrorCode.load(std::memory_order_acquire);
    }

    bool register_input_capture_sink(const InputCaptureSink* sink) noexcept {
        if (!sink || !sink->callback) return false;
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        m_inputSink.exchange(sink, std::memory_order_acq_rel);
        wait_for_callbacks_to_quiesce();
        return true;
    }

    void unregister_input_capture_sink() noexcept {
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        m_inputSink.exchange(nullptr, std::memory_order_acq_rel);
        wait_for_callbacks_to_quiesce();
    }

private:
    static constexpr uint32_t kMaxFramesPerSlice = 8192;

    static constexpr UInt32 kMaxInputChannels = 2;

    void wait_for_callbacks_to_quiesce() noexcept {
        while (m_callbacksInFlight.load(std::memory_order_acquire) != 0)
            std::this_thread::yield();
    }

    static OSStatus deviceListener(AudioObjectID, UInt32, const AudioObjectPropertyAddress[], void* ref) {
        auto* self = static_cast<MacAudioDriver*>(ref);
        if (!self) return noErr;
        self->m_deviceLost.store(true, std::memory_order_release);
        self->m_running.store(false, std::memory_order_release);
        return noErr;
    }

    bool failStart(OSStatus error = -1) {
        m_lastErrorCode.store(static_cast<int32_t>(error), std::memory_order_release);
        stopUnlocked();
        return false;
    }

    AudioUnit m_unit;
    ProcessCallback m_callback;
    mutable std::mutex m_lifecycleMutex;
    double m_sampleRate = 44100.0;
    uint32_t m_bufferSize = 512;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stopping{false};
    std::atomic<uint32_t> m_callbacksInFlight{0};
    std::atomic<bool> m_deviceLost{false};
    std::atomic<int32_t> m_lastErrorCode{0};
    std::atomic<const InputCaptureSink*> m_inputSink{nullptr};
    std::unique_ptr<float[]> m_inputBuffer;
    uint32_t m_inputBufferFrames = 0;
    uint32_t m_inputChannelCount = 0;
    bool m_initialized = false;
    AudioDeviceID m_device = kAudioObjectUnknown;
    bool m_deviceListenerInstalled = false;
};

} // namespace Aura::Core::Driver
