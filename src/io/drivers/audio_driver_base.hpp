#pragma once

#include <string>
#include <vector>
#include <memory>
#include <functional>

namespace Aura::IO::Drivers {

/**
 * @brief IDriver: Professional Hardware Abstraction Layer (HAL) for audio devices.
 * Ensures low-latency communication across different OS APIs (ASIO, CoreAudio, WASAPI).
 */
class IDriver {
public:
    virtual ~IDriver() = default;

    struct Config {
        double sampleRate = 48000.0;
        uint32_t bufferSize = 256;
        uint32_t numInputs = 2;
        uint32_t numOutputs = 2;
    };

    /**
     * @brief Callback function type for processing audio blocks.
     * @param in: Input channels (vector of float pointers)
     * @param out: Output channels (vector of float pointers)
     * @param numFrames: Number of samples per block
     */
    using ProcessCallback = std::function<void(const float** in, float** out, uint32_t numFrames)>;

    virtual bool initialize(const Config& config) = 0;
    virtual bool start(ProcessCallback callback) = 0;
    virtual void stop() = 0;

    virtual std::string getDriverName() const = 0;
    virtual double getSampleRate() const = 0;
    virtual uint32_t getBufferSize() const = 0;
    virtual bool isSilentFallback() const { return false; }
    virtual bool isRunning() const { return false; }
    virtual const std::string& lastError() const {
        static const std::string empty;
        return empty;
    }
};

} // namespace Aura::IO::Drivers
