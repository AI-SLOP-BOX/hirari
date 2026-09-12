#pragma once
#include <cmath>
#include <cstdint>
#include <string>
#include <memory>
#include <functional>
#include <utility>

namespace Aura::IO {

/**
 * @brief IAudioDriver: Physical hardware communication layer.
 */
class IAudioDriver {
public:
    using AudioCallback = std::function<void(float** /*out*/, float** /*in*/, uint32_t /*numFrames*/)>;

    enum class State {
        Uninitialized,
        Initialized,
        Running,
        Stopped,
        Failed
    };

    virtual ~IAudioDriver() = default;
    virtual bool initialize(double sampleRate, uint32_t bufferSize) = 0;
    virtual void start() = 0;
    virtual void stop() = 0;
    virtual std::string getDeviceName() const = 0;

    // Kept separate from start() so the legacy void API remains source-compatible.
    // New code can detect a refused start instead of treating it as success.
    virtual bool startAndReport() {
        start();
        return isRunning();
    }
    virtual State state() const { return State::Uninitialized; }
    virtual bool isInitialized() const { return state() == State::Initialized || state() == State::Running; }
    virtual bool isRunning() const { return state() == State::Running; }
    virtual bool isSilentFallback() const { return false; }
    virtual const std::string& lastError() const {
        static const std::string noError;
        return noError;
    }
    
    void setCallback(AudioCallback cb) { m_callback = std::move(cb); }

protected:
    AudioCallback m_callback;
};

/**
 * @brief DummyAudioDriver: Explicit silent fallback for hardware-less environments.
 *
 * This driver never claims to have opened hardware. Valid silent-mode settings are
 * accepted, while invalid settings and starting before initialization are reported
 * through state()/lastError() and startAndReport().
 */
class DummyAudioDriver : public IAudioDriver {
public:
    static constexpr const char* deviceName() noexcept { return "Aura Silent Engine"; }

    bool initialize(double sr, uint32_t bs) override {
        stop();
        m_sampleRate = sr;
        m_bufferSize = bs;

        if (!(sr > 0.0) || !std::isfinite(sr) || bs == 0) {
            m_state = State::Failed;
            m_lastError = "Silent audio driver requires a finite sample rate and non-zero buffer size";
            return false;
        }

        m_lastError.clear();
        m_state = State::Initialized;
        return true;
    }

    void start() override {
        const State previousState = m_state;
        m_state = State::Failed;
        m_lastError = (previousState == State::Uninitialized)
            ? "Silent audio driver must be initialized before start"
            : "audio backend unavailable; silent fallback cannot start hardware";
    }

    void stop() override {
        if (m_state == State::Initialized || m_state == State::Running || m_state == State::Stopped)
            m_state = State::Stopped;
    }

    bool startAndReport() override {
        start();
        return m_state == State::Running;
    }

    State state() const override { return m_state; }
    bool isInitialized() const override {
        return m_state == State::Initialized || m_state == State::Running || m_state == State::Stopped;
    }
    bool isRunning() const override { return m_state == State::Running; }
    const std::string& lastError() const override { return m_lastError; }
    std::string getDeviceName() const override { return deviceName(); }
    double sampleRate() const noexcept { return m_sampleRate; }
    uint32_t bufferSize() const noexcept { return m_bufferSize; }
    bool isSilentFallback() const override { return true; }

private:
    State m_state = State::Uninitialized;
    double m_sampleRate = 0.0;
    uint32_t m_bufferSize = 0;
    std::string m_lastError;
};

/**
 * @brief Legacy compatibility facade for the canonical drivers::DriverFactory.
 *
 * New code must use `src/io/drivers/audio_driver_factory.hpp`. This older
 * interface is retained only for source compatibility and deliberately
 * returns the silent fallback; it must not be used to claim hardware I/O.
 */
class DriverFactory {
public:
    static std::unique_ptr<IAudioDriver> createSilentFallback() {
        return std::make_unique<DummyAudioDriver>();
    }

    static std::unique_ptr<IAudioDriver> createDefault() {
#ifdef __APPLE__
        // The legacy default remains silent. HardwareFactory in audio_interface.hpp
        // owns the separate CoreAudio implementation.
        return createSilentFallback();
#elif defined(_WIN32)
        return createSilentFallback();
#else
        return createSilentFallback();
#endif
    }
};

} // namespace Aura::IO
