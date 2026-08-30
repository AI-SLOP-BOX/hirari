#include "mac_audio_driver_host.hpp"
#include "mac_audio_driver.hpp"
#include "aura_unified_engine.hpp"
#include "../log_buffer.hpp"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <sstream>

namespace {

constexpr double kFallbackSampleRate = 44100.0;
constexpr uint32_t kFallbackBufferSize = 512;
constexpr uint32_t kAudioLogComponent = 0xA001;

void logAudioMessage(uint32_t level, const char* message) {
    Aura::Core::Diagnostics::LogBuffer::post(level, kAudioLogComponent, message);
}

bool readDeviceProperty(AudioDeviceID device,
                        AudioObjectPropertySelector selector,
                        AudioObjectPropertyScope scope,
                        void* value,
                        UInt32 valueSize) {
    AudioObjectPropertyAddress address = {
        selector,
        scope,
        kAudioObjectPropertyElementMain
    };
    UInt32 size = valueSize;
    return AudioObjectGetPropertyData(device, &address, 0, nullptr, &size, value) == noErr &&
           size >= valueSize;
}

struct AudioDeviceSettings {
    double sampleRate = kFallbackSampleRate;
    uint32_t bufferSize = kFallbackBufferSize;
};

AudioDeviceSettings getDefaultOutputSettings() {
    AudioDeviceSettings settings;
    AudioDeviceID device = kAudioObjectUnknown;
    AudioObjectPropertyAddress defaultDeviceAddress = {
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    UInt32 deviceSize = sizeof(device);
    OSStatus status = AudioObjectGetPropertyData(kAudioObjectSystemObject,
                                                  &defaultDeviceAddress,
                                                  0,
                                                  nullptr,
                                                  &deviceSize,
                                                  &device);
    if (status != noErr || device == kAudioObjectUnknown) {
        char message[96];
        std::snprintf(message, sizeof(message),
                      "Audio device query failed (status=%d); using %g Hz/%u frames",
                      static_cast<int>(status), settings.sampleRate, settings.bufferSize);
        logAudioMessage(1, message);
        return settings;
    }

    Float64 sampleRate = 0.0;
    if (readDeviceProperty(device,
                           kAudioDevicePropertyNominalSampleRate,
                           kAudioObjectPropertyScopeGlobal,
                           &sampleRate,
                           sizeof(sampleRate)) &&
        std::isfinite(sampleRate) && sampleRate > 0.0) {
        settings.sampleRate = sampleRate;
    } else {
        logAudioMessage(1, "Audio sample-rate query failed; using 44100 Hz");
    }

    UInt32 bufferSize = 0;
    if (readDeviceProperty(device,
                           kAudioDevicePropertyBufferFrameSize,
                           kAudioObjectPropertyScopeGlobal,
                           &bufferSize,
                           sizeof(bufferSize)) &&
        bufferSize > 0) {
        settings.bufferSize = bufferSize;
    } else {
        logAudioMessage(1, "Audio buffer-size query failed; using 512 frames");
    }

    char message[96];
    std::snprintf(message, sizeof(message), "Audio device settings: %g Hz/%u frames",
                  settings.sampleRate, settings.bufferSize);
    logAudioMessage(0, message);
    return settings;
}

} // namespace

namespace Aura::Core::Driver {

struct MacAudioDriverHost::Impl {
    struct CallbackState {
        std::atomic<float> outputPeak{0.0f};
        std::atomic<uint64_t> callbackCount{0};
    };

    static void captureInput(void* context,
                             const float* const* channels,
                             uint32_t channelCount,
                             uint32_t frameCount,
                             const AudioTimeStamp* timestamp) noexcept {
        auto* self = static_cast<Impl*>(context);
        if (!self) return;

        // This is the complete host-owned realtime input path: fixed planar
        // storage plus atomics. It never calls the engine, Rust, or CXX.
        self->inputQueue.push_planar(channels, channelCount, frameCount);

        // Preserve the pre-existing optional raw sink as an explicitly
        // registered realtime-safe C++ callback. It is not used by the queue.
        const auto* sink = self->inputSink.load(std::memory_order_acquire);
        if (sink && sink->callback)
            sink->callback(sink->context, channels, channelCount, frameCount, timestamp);
    }

    mutable std::mutex lifecycleMutex;
    std::unique_ptr<MacAudioDriver> driver;
    AudioDeviceID selectedDevice = kAudioObjectUnknown;
    std::shared_ptr<CallbackState> callbackState = std::make_shared<CallbackState>();
    MacAudioInputBlockQueue inputQueue;
    InputCaptureSink queueSink{&captureInput, this};
    std::atomic<const MacAudioDriver::InputCaptureSink*> inputSink{nullptr};
    std::atomic<bool> reconnectRequested{false};
    std::atomic<uint8_t> state{0}; // 0 unavailable, 1 initialized, 2 running, 3 start-failed, 4 stopped
    std::atomic<int32_t> lastErrorCode{0};
    bool defaultDeviceListenerInstalled = false;
};

OSStatus MacAudioDriverHost::defaultDeviceListener(AudioObjectID, UInt32,
                                                   const AudioObjectPropertyAddress[], void* ref) {
    auto* host = static_cast<MacAudioDriverHost::Impl*>(ref);
    if (host) host->reconnectRequested.store(true, std::memory_order_release);
    return noErr;
}

MacAudioDriverHost::MacAudioDriverHost() {
    m_impl = std::make_unique<Impl>();
    AudioObjectPropertyAddress address = {
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    m_impl->defaultDeviceListenerInstalled =
        AudioObjectAddPropertyListener(kAudioObjectSystemObject, &address,
                                       defaultDeviceListener, m_impl.get()) == noErr;
}

MacAudioDriverHost::~MacAudioDriverHost() {
    stop();
    if (m_impl && m_impl->defaultDeviceListenerInstalled) {
        AudioObjectPropertyAddress address = {
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain
        };
        AudioObjectRemovePropertyListener(kAudioObjectSystemObject, &address,
                                          defaultDeviceListener, m_impl.get());
        m_impl->defaultDeviceListenerInstalled = false;
    }
}

bool MacAudioDriverHost::start() {
    if (!m_impl) return false;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    return start_locked();
}

bool MacAudioDriverHost::start_locked() {
    const AudioDeviceSettings settings = getDefaultOutputSettings();
    return start_locked(settings.sampleRate, settings.bufferSize);
}

bool MacAudioDriverHost::start_locked(double sampleRate, uint32_t bufferSize) {
    if (m_impl->driver && m_impl->driver->is_running()) {
        m_impl->state.store(2, std::memory_order_release);
        return true;
    }
    if (m_impl->driver) {
        m_impl->driver->stop();
        m_impl->driver.reset();
    }
    {
        const auto callbackState = m_impl->callbackState;
        m_impl->driver = std::make_unique<MacAudioDriver>([callbackState](float* l, float* r, uint32_t len) {
            float* channels[2] = { l, r };
            ::Aura::Core::Engine::AuraUnifiedEngine::getInstance().processBlockDirect(channels, 2, len);
            float peak = 0.0f;
            for (uint32_t i = 0; i < len; ++i) {
                peak = std::max(peak, std::max(std::fabs(l[i]), std::fabs(r[i])));
            }
            callbackState->outputPeak.store(peak, std::memory_order_release);
            callbackState->callbackCount.fetch_add(1, std::memory_order_relaxed);
        });
        m_impl->driver->register_input_capture_sink(&m_impl->queueSink);
        m_impl->state.store(1, std::memory_order_release);
        ::Aura::Core::Engine::AuraUnifiedEngine::getInstance().prepareToPlay(
            sampleRate, bufferSize);

        // Keep the driver and Engine on the exact requested configuration.
        if (!m_impl->driver->start(sampleRate, bufferSize, m_impl->selectedDevice)) {
            const int32_t errorCode = static_cast<int32_t>(m_impl->driver->last_error_code());
            m_impl->lastErrorCode.store(errorCode, std::memory_order_release);
            char message[128];
            std::snprintf(message, sizeof(message),
                          "Audio driver start failed (OSStatus=%d)",
                          static_cast<int>(errorCode));
            logAudioMessage(1, message);
            m_impl->state.store(3, std::memory_order_release);
            m_impl->driver.reset();
            // A failed restart must not expose blocks captured by the
            // previous device configuration to the next recording session.
            // The driver has already quiesced its callback before reset().
            m_impl->inputQueue.reset();
            return false;
        } else {
            m_impl->lastErrorCode.store(0, std::memory_order_release);
            m_impl->state.store(2, std::memory_order_release);
            return true;
        }
    }
}

bool MacAudioDriverHost::select_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) {
    if (!m_impl || deviceId == kAudioObjectUnknown || !std::isfinite(sampleRate) || sampleRate <= 0.0 || bufferSize == 0) return false;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    const AudioDeviceID previousDevice = m_impl->selectedDevice;
    m_impl->selectedDevice = static_cast<AudioDeviceID>(deviceId);
    stop_locked();
    if (start_locked(sampleRate, bufferSize)) return true;

    // A device can disappear between enumeration and AudioUnit creation.
    // Restore the previous selection and callback boundary before reporting
    // failure, so the caller never leaves the session without its last-known
    // working device merely because a transient switch failed.
    m_impl->selectedDevice = previousDevice;
    return start_locked(sampleRate, bufferSize);
}

void MacAudioDriverHost::try_reconnect() {
    if (!m_impl) return;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    const bool requested = m_impl->reconnectRequested.exchange(false, std::memory_order_acq_rel);
    const bool running = m_impl->driver && m_impl->driver->is_running();
    if (requested || !running) {
        stop_locked();
        start_locked();
    }
}

bool MacAudioDriverHost::reconfigure(double sampleRate, uint32_t bufferSize) {
    if (!m_impl || !std::isfinite(sampleRate) || sampleRate <= 0.0 || bufferSize == 0) {
        return false;
    }
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    stop_locked();
    return start_locked(sampleRate, bufferSize);
}

std::string MacAudioDriverHost::list_devices_json() const {
    AudioObjectPropertyAddress address = {
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, nullptr, &size) != noErr ||
        size == 0 || size > 1024u * sizeof(AudioDeviceID)) return "[]";
    std::vector<AudioDeviceID> devices(size / sizeof(AudioDeviceID));
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, nullptr, &size, devices.data()) != noErr) return "[]";
    std::ostringstream json;
    json << '[';
    bool first = true;
    for (const AudioDeviceID device : devices) {
        CFStringRef name = nullptr;
        AudioObjectPropertyAddress nameAddress = {
            kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain
        };
        UInt32 nameSize = sizeof(name);
        if (AudioObjectGetPropertyData(device, &nameAddress, 0, nullptr, &nameSize, &name) != noErr || !name) continue;
        char nameBuffer[512] = {};
        const bool converted = CFStringGetCString(name, nameBuffer, sizeof(nameBuffer), kCFStringEncodingUTF8);
        CFRelease(name);
        if (!converted) continue;
        auto hasScope = [device](AudioObjectPropertyScope scope) {
            AudioObjectPropertyAddress streamAddress = {
                kAudioDevicePropertyStreams, scope, kAudioObjectPropertyElementMain
            };
            UInt32 streamSize = 0;
            return AudioObjectGetPropertyDataSize(device, &streamAddress, 0, nullptr, &streamSize) == noErr && streamSize > 0;
        };
        if (!first) json << ',';
        first = false;
        json << "{\"id\":" << device << ",\"name\":\"";
        for (const char* c = nameBuffer; *c; ++c) {
            if (*c == '\\' || *c == '"') json << '\\';
            json << *c;
        }
        json << "\",\"input\":" << (hasScope(kAudioObjectPropertyScopeInput) ? "true" : "false")
             << ",\"output\":" << (hasScope(kAudioObjectPropertyScopeOutput) ? "true" : "false") << '}';
    }
    json << ']';
    return json.str();
}

void MacAudioDriverHost::stop() {
    if (!m_impl) return;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    stop_locked();
}

void MacAudioDriverHost::stop_locked() {
    // Device loss is a transport boundary. Stop the graph before tearing down
    // the callback so a callback-free interval cannot leave the engine marked
    // as playing and accidentally resume against a new device configuration.
    ::Aura::Core::Engine::AuraUnifiedEngine::getInstance().set_playing(false);
    if (!m_impl->driver) {
        if (m_impl->state.load(std::memory_order_acquire) == 2) {
            m_impl->state.store(4, std::memory_order_release);
        }
        return;
    }
    m_impl->driver->stop();
    m_impl->driver.reset();
    m_impl->inputQueue.reset();
    m_impl->state.store(4, std::memory_order_release);
}

bool MacAudioDriverHost::is_running() const {
    if (!m_impl) return false;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    return m_impl->driver && m_impl->driver->is_running();
}

const char* MacAudioDriverHost::status() const noexcept {
    if (!m_impl) return "unavailable";
    switch (m_impl->state.load(std::memory_order_acquire)) {
        case 1: return "initialized";
        case 2: return "running";
        case 3: return "start-failed";
        case 4: return "stopped";
        default: return "unavailable";
    }
}

int32_t MacAudioDriverHost::last_error_code() const noexcept {
    if (!m_impl) return 0;
    if (m_impl->driver) {
        return static_cast<int32_t>(m_impl->driver->last_error_code());
    }
    return m_impl->lastErrorCode.load(std::memory_order_acquire);
}

float MacAudioDriverHost::output_peak() const {
    if (!m_impl) return 0.0f;
    const auto callbackState = m_impl->callbackState;
    return callbackState->outputPeak.load(std::memory_order_acquire);
}

uint64_t MacAudioDriverHost::callback_count() const {
    if (!m_impl) return 0;
    const auto callbackState = m_impl->callbackState;
    return callbackState->callbackCount.load(std::memory_order_acquire);
}

bool MacAudioDriverHost::poll_input_block(float* const* destination,
                                          uint32_t destinationChannelCapacity,
                                          uint32_t destinationFrameCapacity,
                                          InputBlockInfo& info,
                                          uint64_t& droppedBlocks) noexcept {
    if (!m_impl) {
        info = {};
        droppedBlocks = 0;
        return false;
    }
    return m_impl->inputQueue.poll(destination, destinationChannelCapacity,
                                   destinationFrameCapacity, info, droppedBlocks);
}

uint64_t MacAudioDriverHost::dropped_input_blocks() const noexcept {
    return m_impl ? m_impl->inputQueue.dropped_blocks() : 0;
}

bool MacAudioDriverHost::register_input_capture_sink(const InputCaptureSink* sink) noexcept {
    if (!m_impl || !sink || !sink->callback) return false;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    m_impl->inputSink.store(sink, std::memory_order_release);
    return true;
}

void MacAudioDriverHost::unregister_input_capture_sink() noexcept {
    if (!m_impl) return;
    std::lock_guard<std::mutex> lock(m_impl->lifecycleMutex);
    m_impl->inputSink.store(nullptr, std::memory_order_release);
    if (m_impl->driver)
        m_impl->driver->register_input_capture_sink(&m_impl->queueSink);
}

} // namespace Aura::Core::Driver
