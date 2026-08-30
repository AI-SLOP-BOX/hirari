#pragma once

#include <cstdint>
#include <cmath>
#include <memory>
#include <string>

namespace Aura::Platform {

/** Platform-neutral real-time audio device contract. */
class AudioDevice {
public:
    using Callback = void (*)(void* user, float** outputs, const float** inputs, uint32_t frames) noexcept;

    struct Config {
        constexpr Config(double sr = 48000.0, uint32_t bs = 256,
                         uint32_t in = 2, uint32_t out = 2) noexcept
            : sampleRate(sr), bufferSize(bs), inputs(in), outputs(out) {}
        double sampleRate;
        uint32_t bufferSize;
        uint32_t inputs;
        uint32_t outputs;
    };

    virtual ~AudioDevice() = default;
    virtual bool initialize(const Config& config, Callback callback, void* user) noexcept = 0;
    virtual bool start() noexcept = 0;
    virtual void stop() noexcept = 0;
    virtual bool isRunning() const noexcept = 0;
    virtual const char* name() const noexcept = 0;

    // Added as non-breaking capability queries so callers do not mistake a
    // silent fallback for an opened hardware device.
    virtual bool isHardwareAvailable() const noexcept { return true; }
    virtual bool isSilentFallback() const noexcept { return false; }
    virtual const char* lastError() const noexcept { return ""; }
    virtual bool hasError() const noexcept { return lastError()[0] != '\0'; }
    virtual Config config() const noexcept { return Config{}; }

protected:
};

/** Safe fallback used when a native backend is unavailable. */
class SilentAudioDevice final : public AudioDevice {
public:
    bool initialize(const Config& config, Callback callback, void* user) noexcept override {
        m_initialized = false;
        m_config = config;
        m_callback = callback;
        m_user = user;
        m_error.clear();
        m_initialized = std::isfinite(config.sampleRate) && config.sampleRate > 0.0 &&
                        config.bufferSize > 0 && config.outputs > 0;
        if (!m_initialized) m_error = "invalid audio device configuration";
        return m_initialized;
    }
    bool start() noexcept override {
        m_running = false;
        m_error = "audio backend unavailable; silent fallback selected";
        return false;
    }
    void stop() noexcept override { m_running = false; }
    bool isRunning() const noexcept override { return m_running; }
    const char* name() const noexcept override { return "Aura Silent Audio Device"; }
    bool isHardwareAvailable() const noexcept override { return false; }
    bool isSilentFallback() const noexcept override { return true; }
    const char* lastError() const noexcept override {
        return m_error.c_str();
    }
    Config config() const noexcept override { return m_config; }

private:
    Config m_config{};
    Callback m_callback = nullptr;
    void* m_user = nullptr;
    bool m_initialized = false;
    bool m_running = false;
    std::string m_error = "audio backend unavailable; silent fallback selected";
};

#if defined(__APPLE__)
std::unique_ptr<AudioDevice> createAudioDevice();
#else
inline std::unique_ptr<AudioDevice> createAudioDevice() {
    return std::make_unique<SilentAudioDevice>();
}
#endif

} // namespace Aura::Platform
