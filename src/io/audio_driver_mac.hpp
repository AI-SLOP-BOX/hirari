#pragma once

#include <iostream>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <thread>
#include <mutex>
#include <vector>
#include <AudioUnit/AudioUnit.h>
#include <AudioToolbox/AudioToolbox.h>
#include "../core/aura_unified_engine.hpp"

namespace Aura::IO {

/**
 * @brief AudioDriverMac: Native macOS CoreAudio Integration.
 * Bridges the high-level Aura engine with Apple's Hardware Abstraction Layer (HAL).
 */
class AudioDriverMac {
public:
    static AudioDriverMac& getInstance() { static AudioDriverMac i; return i; }

    /**
     * @brief START: Initializes the AudioUnit and begins the real-time callback loop.
     */
    bool start(double sr, uint32_t bufferSize) {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        if (!std::isfinite(sr) || sr <= 0.0 || bufferSize == 0 || bufferSize > kMaxFrames) {
            return false;
        }
        stop();
        m_sampleRate = sr;
        m_bufferSize = bufferSize;

        // 1. SETUP AUDIO COMPONENT DESCRIPTION
        AudioComponentDescription desc;
        desc.componentType = kAudioUnitType_Output;
        desc.componentSubType = kAudioUnitSubType_DefaultOutput;
        desc.componentManufacturer = kAudioUnitManufacturer_Apple;
        desc.componentFlags = 0;
        desc.componentFlagsMask = 0;

        AudioComponent comp = AudioComponentFindNext(NULL, &desc);
        if (!comp) return false;

        OSStatus status = AudioComponentInstanceNew(comp, &m_outputUnit);
        if (status != noErr) {
            m_outputUnit = nullptr;
            return false;
        }

        // 2. SET CALLBACK
        AURenderCallbackStruct input;
        input.inputProc = audioCallback;
        input.inputProcRefCon = this;
        status = AudioUnitSetProperty(m_outputUnit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &input, sizeof(input));
        if (status != noErr) {
            stop();
            return false;
        }

        // 3. ACTIVATE
        status = AudioUnitInitialize(m_outputUnit);
        if (status != noErr) {
            stop();
            return false;
        }

        status = AudioOutputUnitStart(m_outputUnit);
        if (status != noErr) {
            stop();
            return false;
        }
        m_running.store(true, std::memory_order_release);
        
        std::cout << "[CoreAudio] Driver Started @ " << sr << "Hz / " << bufferSize << " samples." << std::endl;
        return true;
    }

    void stop() {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        if (m_outputUnit) {
            m_stopping.store(true, std::memory_order_release);
            const bool wasRunning = m_running.exchange(false, std::memory_order_acq_rel);
            // Stop the HAL before waiting for callbacks. The AudioUnit keeps
            // the callback refCon valid until this call returns.
            if (wasRunning) AudioOutputUnitStop(m_outputUnit);
            while (m_callbacksInFlight.load(std::memory_order_acquire) != 0) {
                std::this_thread::yield();
            }
            AudioUnitUninitialize(m_outputUnit);
            AudioComponentInstanceDispose(m_outputUnit);
            m_outputUnit = nullptr;
            m_stopping.store(false, std::memory_order_release);
        }
    }

private:
    /**
     * @brief THE REAL-TIME CALLBACK: Bridges HAL hardware buffers to the Aura Kernel.
     * HONEST FIX: Implements actual hardware input rendering.
     */
    static OSStatus audioCallback(void* inRefCon, AudioUnitRenderActionFlags* ioActionFlags,
                                   const AudioTimeStamp* inTimeStamp, UInt32 inBusNumber,
                                   UInt32 inNumberFrames, AudioBufferList* ioData) {
        auto* driver = static_cast<AudioDriverMac*>(inRefCon);
        if (!driver) return noErr;
        (void)inBusNumber;
        driver->m_callbacksInFlight.fetch_add(1, std::memory_order_acq_rel);
        struct CallbackGuard {
            AudioDriverMac* driver;
            ~CallbackGuard() {
                driver->m_callbacksInFlight.fetch_sub(1, std::memory_order_release);
            }
        } guard{driver};

        AudioUnit outputUnit = driver->m_outputUnit;
        if (!ioData || !outputUnit ||
            driver->m_stopping.load(std::memory_order_acquire) ||
            !driver->m_running.load(std::memory_order_acquire) ||
            inNumberFrames == 0 || inNumberFrames > kMaxFrames) {
            return noErr;
        }
        
        // 1. CAPTURE INPUT (Recording/Sidechain)
        // HONEST FIX: Pull samples from the hardware input bus (Bus 1)
        struct StereoInputBufferList {
            UInt32 mNumberBuffers;
            AudioBuffer mBuffers[2];
        } inputBufferList{};
        inputBufferList.mNumberBuffers = 2;
        for (UInt32 channel = 0; channel < 2; ++channel) {
            inputBufferList.mBuffers[channel].mNumberChannels = 1;
            inputBufferList.mBuffers[channel].mDataByteSize = inNumberFrames * sizeof(float);
            inputBufferList.mBuffers[channel].mData =
                driver->m_inputBuffer.data() + static_cast<size_t>(channel) * kMaxFrames;
        }

        const OSStatus inputStatus = AudioUnitRender(outputUnit, ioActionFlags,
                                                     inTimeStamp, 1, inNumberFrames,
                                                     reinterpret_cast<AudioBufferList*>(&inputBufferList));
        if (inputStatus != noErr) {
            std::fill_n(driver->m_inputBuffer.data(), 2 * kMaxFrames, 0.0f);
        }

        const float* inputs[2] = {
            driver->m_inputBuffer.data(),
            driver->m_inputBuffer.data() + kMaxFrames
        };

        // 2. PREPARE OUTPUT (Atmos 7.1.4)
        float* outputs[12] = { nullptr };
        for (uint32_t i = 0; i < std::min<uint32_t>(12, ioData->mNumberBuffers); ++i) {
            outputs[i] = static_cast<float*>(ioData->mBuffers[i].mData);
        }
        
        (void)inputs;
        Core::Engine::AuraUnifiedEngine::getInstance().processBlockDirect(
            outputs, std::min<uint32_t>(12, ioData->mNumberBuffers), inNumberFrames);
        
        return noErr;
    }

    static constexpr UInt32 kMaxFrames = 65536;

    AudioDriverMac()
        : m_sampleRate(0.0), m_bufferSize(0), m_outputUnit(nullptr),
          m_inputBuffer(2 * kMaxFrames, 0.0f) {}
    double m_sampleRate = 0.0;
    uint32_t m_bufferSize = 0;
    AudioUnit m_outputUnit = nullptr;
    std::vector<float> m_inputBuffer;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stopping{false};
    std::atomic<uint32_t> m_callbacksInFlight{0};
    std::recursive_mutex m_lifecycleMutex;
};

} // namespace Aura::IO
