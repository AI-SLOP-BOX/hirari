#pragma once

#include "audio_driver_base.hpp"
#include <cmath>
#include <memory>
#include <string>
#include <utility>

namespace Aura::IO::Drivers {

/**
 * @brief DriverFactory: Platform-aware factory for audio drivers.
 * Provides a Null-ptr safe mechanism to retrieve the best available audio API.
 */
class DriverFactory {
public:
    enum class API { Auto, ASIO, CoreAudio, WASAPI, PipeWire, Dummy };

    /**
     * @brief Creates the best driver for the current system.
     * @param api: Preferred API. Defaults to platform-native.
     * @return A valid IDriver instance. If requested API is unavailable, returns a Silent/Dummy driver.
     */
    /**
     * @brief SilentDriver: A safe fallback that produces no sound but prevents engine crashes.
     */
    class SilentDriver : public IDriver {
    public:
        explicit SilentDriver(std::string reason =
            "audio backend unavailable; silent fallback selected")
            : m_error(std::move(reason)) {}

        bool initialize(const Config& config) override {
            if (!std::isfinite(config.sampleRate) || config.sampleRate <= 0.0 || config.bufferSize == 0) {
                m_error = "silent driver requires a finite sample rate and non-zero buffer size";
                m_initialized = false;
                return false;
            }
            m_config = config;
            m_initialized = true;
            m_running = false;
            return true;
        }
        bool start(ProcessCallback callback) override {
            if (!m_initialized) {
                m_error = "silent driver must be initialized before start";
                return false;
            }
            m_callback = std::move(callback);
            m_running = false;
            if (m_error.empty()) {
                m_error = "requested audio backend is unavailable; using silent fallback";
            }
            return false;
        }
        void stop() override { m_running = false; }
        std::string getDriverName() const override {
            return "Aura Silent Fallback (audio backend unavailable)";
        }
        double getSampleRate() const override { return m_config.sampleRate; }
        uint32_t getBufferSize() const override { return m_config.bufferSize; }
        bool isSilentFallback() const override { return true; }
        bool isRunning() const override { return m_running; }
        const std::string& lastError() const override { return m_error; }
    private:
        Config m_config;
        ProcessCallback m_callback;
        std::string m_error;
        bool m_initialized = false;
        bool m_running = false;
    };

    /**
     * @brief Creates the currently available driver implementation.
     *
     * Native WASAPI/ASIO/PipeWire/CoreAudio adapters live in separate platform
     * layers and are not linked into this legacy facade yet. Returning the
     * explicit silent driver keeps the factory total and prevents callers from
     * treating an unimplemented backend as a real audio device.
     */
    static std::unique_ptr<IDriver> create(API api = API::Auto);

    static const char* apiName(API api) noexcept {
        switch (api) {
            case API::Auto: return "Auto";
            case API::ASIO: return "ASIO";
            case API::CoreAudio: return "CoreAudio";
            case API::WASAPI: return "WASAPI";
            case API::PipeWire: return "PipeWire";
            case API::Dummy: return "Dummy";
        }
        return "Unknown";
    }
};

} // namespace Aura::IO::Drivers

#if defined(__APPLE__)
#include "../../core/driver/mac_audio_driver.hpp"

namespace Aura::IO::Drivers {

class MacDriverBridge : public IDriver {
public:
    MacDriverBridge() : m_running(false) {}

    bool initialize(const Config& config) override {
        if (!std::isfinite(config.sampleRate) || config.sampleRate <= 0.0 ||
            config.sampleRate > 384000.0 || config.bufferSize == 0 ||
            config.bufferSize > 8192 || config.numOutputs == 0 ||
            config.numOutputs > 2) {
            m_error = "invalid CoreAudio configuration";
            return false;
        }
        m_config = config;
        
        m_driver = std::make_unique<::Aura::Core::Driver::MacAudioDriver>([this](float* l, float* r, uint32_t len) {
            if (m_callback) {
                float* outChans[2] = { l, r };
                m_callback(nullptr, outChans, len);
            }
        });
        return m_driver != nullptr;
    }

    bool start(ProcessCallback callback) override {
        m_callback = std::move(callback);
        if (m_driver) {
            m_running = m_driver->start(m_config.sampleRate, m_config.bufferSize);
            if (!m_running) m_error = "CoreAudio failed to start the requested device";
            return m_running;
        }
        m_error = "CoreAudio driver was not initialized";
        return false;
    }

    void stop() override {
        if (m_driver) {
            m_driver->stop();
        }
        m_running = false;
    }

    std::string getDriverName() const override {
        return "Aura CoreAudio Driver";
    }

    double getSampleRate() const override { return m_config.sampleRate; }
    uint32_t getBufferSize() const override { return m_config.bufferSize; }
    bool isSilentFallback() const override { return false; }
    bool isRunning() const override { return m_running; }
    const std::string& lastError() const override { return m_error; }

private:
    Config m_config;
    ProcessCallback m_callback;
    std::unique_ptr<::Aura::Core::Driver::MacAudioDriver> m_driver;
    bool m_running;
    std::string m_error;
};

inline std::unique_ptr<IDriver> DriverFactory::create(API api) {
    if (api == API::Auto || api == API::CoreAudio) {
        auto drv = std::make_unique<MacDriverBridge>();
        IDriver::Config cfg;
        if (drv->initialize(cfg)) {
            return drv;
        }
    }
    return std::make_unique<SilentDriver>(
        std::string("requested audio API is unavailable: ") + apiName(api));
}

} // namespace Aura::IO::Drivers
#else
namespace Aura::IO::Drivers {
inline std::unique_ptr<IDriver> DriverFactory::create(API api) {
    return std::make_unique<SilentDriver>(
        std::string("audio API is not implemented on this platform: ") + apiName(api));
}
} // namespace Aura::IO::Drivers
#endif
