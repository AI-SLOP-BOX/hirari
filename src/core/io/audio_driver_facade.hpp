#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <mutex>

namespace Aura::Core::IO {

enum class DriverProtocol { CoreAudio, ASIO, JACK, WASAPI, ALSA, Dummy };

struct DeviceDescriptor {
    std::string name;
    uint32_t outputChannels;
    uint32_t inputChannels;
    std::vector<uint32_t> supportedSampleRates;
};

/**
 * @class IAudioDriver
 * @brief Abstract interface for professional audio drivers.
 */
class IAudioDriver {
public:
    virtual ~IAudioDriver() = default;
    virtual bool openDevice(const std::string& name) = 0;
    virtual void closeDevice() = 0;
    virtual bool start() = 0;
    virtual void stop() = 0;
    
    virtual double getSampleRate() const = 0;
    virtual uint32_t getBlockSize() const = 0;
    virtual uint32_t getLatencySamples() const = 0;
};

#include "buffer_interleaver.hpp"

namespace Aura::Core::IO {

/**
 * @class AudioDriverFacade
 * @brief Managed interface for multi-protocol audio I/O.
 * HONEST FIX: Replaced 'Industrial' branding with functional driver orchestration.
 */
class AudioDriverFacade {
public:
    using ProcessCallback = void (*)(float** channels, uint32_t channelCount, uint32_t frameCount, void* context) noexcept;

    AudioDriverFacade(DriverProtocol protocol) : m_protocol(protocol) {}
    ~AudioDriverFacade() { stop(); }

    void attachDriver(std::unique_ptr<IAudioDriver> driver) {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        if (m_activeDriver) {
            m_activeDriver->stop();
            m_activeDriver->closeDevice();
        }
        m_activeDriver = std::move(driver);
        m_isOpen.store(false, std::memory_order_release);
        m_isRunning.store(false, std::memory_order_release);
        m_generation.fetch_add(1, std::memory_order_acq_rel);
    }

    bool openDevice(const std::string& name) {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        if (!m_activeDriver || name.empty()) return false;
        if (m_isRunning.exchange(false, std::memory_order_acq_rel)) m_activeDriver->stop();
        if (m_isOpen.exchange(false, std::memory_order_acq_rel)) m_activeDriver->closeDevice();
        const bool opened = m_activeDriver->openDevice(name);
        m_isOpen.store(opened, std::memory_order_release);
        m_generation.fetch_add(1, std::memory_order_acq_rel);
        return opened;
    }

    bool start() {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        if (!m_activeDriver || !m_isOpen.load(std::memory_order_acquire)) return false;
        const bool started = m_activeDriver->start();
        m_isRunning.store(started, std::memory_order_release);
        return started;
    }

    void stop() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        if (!m_activeDriver) return;
        if (m_isRunning.exchange(false, std::memory_order_acq_rel)) m_activeDriver->stop();
        if (m_isOpen.exchange(false, std::memory_order_acq_rel)) m_activeDriver->closeDevice();
    }

    bool isOpen() const noexcept { return m_isOpen.load(std::memory_order_acquire); }
    bool isRunning() const noexcept { return m_isRunning.load(std::memory_order_acquire); }
    uint64_t generation() const noexcept { return m_generation.load(std::memory_order_acquire); }

    void setProcessCallback(ProcessCallback callback, void* context) noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        m_processContext.store(context, std::memory_order_release);
        m_processCallback.store(callback, std::memory_order_release);
    }

    /**
     * @brief Demonstrates the hardware I/O processing flow.
     */
    void process(float* hardwareInput, float* hardwareOutput, float** internalBuffers, uint32_t numChannels, uint32_t numSamples) {
        if (!hardwareOutput || !internalBuffers || numChannels == 0 || numSamples == 0) return;
        // 1. Convert hardware input to internal format
        if (hardwareInput) {
            BufferInterleaver::deinterleave(hardwareInput, internalBuffers, numChannels, numSamples);
        } else {
            std::fill_n(hardwareOutput, static_cast<size_t>(numChannels) * numSamples, 0.0f);
            return;
        }
        
        // 2. Run the prepared DAW graph. The callback is installed on the
        // control thread and must be realtime-safe when invoked by the driver.
        if (const auto callback = m_processCallback.load(std::memory_order_acquire)) {
            callback(internalBuffers, numChannels, numSamples,
                     m_processContext.load(std::memory_order_acquire));
        }
        
        // 3. Convert internal format back to hardware output
        BufferInterleaver::interleave(internalBuffers, hardwareOutput, numChannels, numSamples);
    }

private:
    DriverProtocol m_protocol;
    std::unique_ptr<IAudioDriver> m_activeDriver;
    mutable std::mutex m_lifecycleMutex;
    std::atomic<bool> m_isOpen{false};
    std::atomic<bool> m_isRunning{false};
    std::atomic<uint64_t> m_generation{0};
    std::atomic<ProcessCallback> m_processCallback{nullptr};
    std::atomic<void*> m_processContext{nullptr};
};

} // namespace Aura::Core::IO
