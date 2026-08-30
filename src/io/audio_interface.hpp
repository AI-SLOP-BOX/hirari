#pragma once

#include "audio_drivers.hpp"
#include "drivers/audio_driver_factory.hpp"

namespace Aura::IO {
    // Deprecated: Use IAudioDriver 
    using IAudioInterface = IAudioDriver;
    using DummyHardware = DummyAudioDriver;

    class LegacyDriverAdapter final : public IAudioInterface {
    public:
        explicit LegacyDriverAdapter(std::unique_ptr<Drivers::IDriver> driver)
            : m_driver(std::move(driver)) {}

        bool initialize(double sampleRate, uint32_t bufferSize) override {
            if (!m_driver) return false;
            Drivers::IDriver::Config config;
            config.sampleRate = sampleRate;
            config.bufferSize = bufferSize;
            return m_driver->initialize(config);
        }

        void start() override {
            if (m_driver) m_driver->start([this](const float** input, float** output, uint32_t frames) {
                if (m_callback) m_callback(output, const_cast<float**>(input), frames);
            });
        }

        bool startAndReport() override {
            if (!m_driver) return false;
            return m_driver->start([this](const float** input, float** output, uint32_t frames) {
                if (m_callback) m_callback(output, const_cast<float**>(input), frames);
            });
        }

        void stop() override { if (m_driver) m_driver->stop(); }
        std::string getDeviceName() const override {
            return m_driver ? m_driver->getDriverName() : "Aura audio driver unavailable";
        }
        State state() const override {
            if (!m_driver) return State::Failed;
            if (m_driver->isRunning()) return State::Running;
            if (m_driver->isSilentFallback()) return State::Failed;
            return State::Initialized;
        }
        bool isInitialized() const override { return m_driver != nullptr; }
        bool isRunning() const override { return m_driver && m_driver->isRunning(); }
        bool isSilentFallback() const override { return !m_driver || m_driver->isSilentFallback(); }
        const std::string& lastError() const override {
            static const std::string unavailable = "audio driver unavailable";
            return m_driver ? m_driver->lastError() : unavailable;
        }

    private:
        std::unique_ptr<Drivers::IDriver> m_driver;
    };

    class HardwareFactory {
    public:
        static std::unique_ptr<IAudioInterface> createDefault();
    };
} // namespace Aura::IO
